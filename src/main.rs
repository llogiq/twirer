use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::{env, fs, io};
use std::io::{BufRead, BufReader, Write};
use octocrab::Octocrab;

static WORKSPACE: &str = "/home/andre/projects/twirer";

fn repo_title(line: &str) -> (&str, &str) {
	line.rsplit_once("](").map_or(("", line), |(title, href)| (href[10..].split('/').nth(2).unwrap_or(""), title))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
	env::set_current_dir(WORKSPACE)?;
	let mut args = env::args();
	let cmd = args.nth(1);
	match cmd.as_ref().map_or("", |s| s) {
		"prs" => {
			// ' is:pr org:rust-lang is:merged merged:YYYY-MM-DD..YYYY-MM-DD'
			let spec = " is:pr org:rust-lang is:merged merged:".to_owned() + 
				&args.next().expect("needs a search spec as second argument");
			let token = env::var("GH_TOKEN")?;
			let octocrab = Octocrab::builder().personal_token(token).build()?;
			let search = octocrab
				.search()
				.issues_and_pull_requests(&spec)
				.per_page(100);
			let mut page = search.send().await?;
			// get and write the total count
			let _ = fs::create_dir_all("cache"); // ignore possible errors
			let total_count = page.total_count.unwrap_or(0);
			{
				let mut out = fs::File::create("cache/num_prs")?;
				writeln!(out, "{} pull rte_dirquests were [merged in the last wek][merged]", total_count)?;
			}
			// get all the PRs
			let mut prs = page.take_items();
			while let Some(mut new_page) = octocrab.get_page(&page.next).await? {
				prs.extend(new_page.take_items());
				page = new_page;
			}
			let repos: HashMap<&'static str, &'static str> = HashMap::from([
				("rust-clippy", "clippy"),
				("rustfmt", "rustfmt"),
				("cargo", "cargo"),
				("rustc_codegen_gcc", "codegen\\_gcc"),
				("futures-rs", "futures"),
				("rustup", "rustup"),
			]);
			{
				let out = fs::File::create("cache/prs")?;
				let mut out = io::BufWriter::new(out);
				for pr in prs {
					let url = pr.html_url;
					if let Some(unprefixed) = url.path().strip_prefix("/rust-lang/") {
						if let Some(repo) = unprefixed.split_once("/") {
							if let Some(reponame) = repos.get(repo.0) {
								if !pr.title.starts_with(reponame) {
									writeln!(out, "* [{}: {}]({})", reponame, pr.title, url)?;
									continue;
								}
							}
						}
					}
					writeln!(out, "* [{}]({})", pr.title.trim_matches(&[' ', '.'][..]), url)?;
				}
			}
		},
		"filter" => {
			let prev = fs::File::open("cache/last_prs")?;
			let prev = BufReader::new(prev);
			let mut previous = HashSet::new();
			for line in prev.lines() {
				let line = line?;
				previous.insert(if let Some((_, r)) = line.rsplit_once("](") {
					r.to_owned()
				} else {
					line
				});
			}
			let prs = fs::File::open("cache/prs")?;
			let mut sorted_prs = Vec::new();
			for pr in BufReader::new(prs).lines() {
				let pr = pr?;
				let (_title, href) = pr.rsplit_once("](").unwrap_or((&pr, ""));
				if previous.contains(href) {
					continue;
				}
				let lower = pr.to_lowercase();
				if ["beta", "bump", "update", "rollup", "typo", "glacier", "surveys", "nomicon",
					"rustlings", "rust-lang/team", "rust-by-example", "this-week-in-rust",
					"blog.rust-lang.org", "long explanation", "missing word", "mailmap",
					"add #[must_use] to", "[rustup]", "lock file maintenance", "spelling",
					":arrow_up:", "arewewebyet", "edition-guide", "www.rust-lang.org",
					"add regression test", "/book/", "/impl-trait-initiative/"]
					.iter()
					.any(|kw| lower.contains(kw))
				{
					continue;
				}
				sorted_prs.push(pr);
			}
			sorted_prs.sort_by(|a, b| repo_title(a).cmp(&repo_title(b))); 
			let mut filtered_prs = fs::File::create("cache/filteredprs")?;
			for pr in sorted_prs {
				writeln!(filtered_prs, "{}", pr)?;
			}
		},
		_ => {
			println!("usage: twirer [prs <spec>|filter]");
		},
	}
	Ok(())
}
