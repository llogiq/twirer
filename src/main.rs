use octocrab::Octocrab;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::Command;
use std::{env, fs, io};
use time::{macros::format_description, Date, Duration};

static WORKSPACE: &str = env!("CARGO_MANIFEST_DIR");
static TWIR: &str = "../this-week-in-rust";

type Config = HashMap<String, String>;

fn read_config() -> Result<Config, Box<dyn Error>> {
    let config = fs::read_to_string("cache/config")?;
    Ok(config
        .split('\n')
        .flat_map(|s| s.trim_end().split_once("="))
        .map(|(k, v)| (k.into(), v.into()))
        .collect())
}

fn get_list<'c>(config: &'c Config, key: &str) -> Result<Vec<&'c str>, Box<dyn Error>> {
    Ok(config
        .get(key)
        .ok_or_else(|| Cow::Owned(format!("missing `{key}=a, b, c` in config")))?
        .split(", ")
        .collect())
}

fn repo_title<'l>(line: &'l str, order: &[&str]) -> (usize, &'l str, &'l str) {
    line.rsplit_once("](")
        .map_or((usize::MAX, "", line), |(title, href)| {
            let repo = href[10..].split('/').nth(2).unwrap_or("");
            (
                order.iter().position(|&r| r == repo).unwrap_or(usize::MAX),
                repo,
                title,
            )
        })
}

fn week() -> Result<String, Box<dyn Error>> {
    let current = fs::read_to_string("cache/week_spec")?;
    let until = current.trim().splitn(2, "..").nth(1).unwrap();
    let date_format = format_description!("[year]-[month]-[day]");
    let date = Date::parse(until, date_format)?;
    let next_date = date + Duration::WEEK;
    let result = format!(
        "{}..{}",
        date.format(date_format)?,
        next_date.format(date_format)?,
    );
    Ok(result)
}

fn token() -> Result<String, Box<dyn Error>> {
    if let Ok(token) = env::var("GH_TOKEN") {
        return Ok(token);
    }
    let mut out = io::stdout();
    out.write(b"token: ")?;
    out.flush()?;
    let mut token = String::new();
    std::io::stdin().read_line(&mut token)?;
    if token.ends_with('\n') {
        let _newline = token.pop();
        if token.ends_with("\r") {
            let _cr = token.pop();
        }
    }
    Ok(token)
}

async fn prs(week_spec: &str) -> Result<u64, Box<dyn Error>> {
    // 'YYYY-MM-DD..YYYY-MM-DD'
    let spec = " is:pr org:rust-lang is:merged merged:".to_owned() + week_spec;
    let token = token()?;
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
        writeln!(
            out,
            "{} pull requests were [merged in the last week][merged]",
            total_count
        )?;
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
        ("libc", "libc"),
        ("docs.rs", "docs.rs"),
        ("hashbrown", "hashbrown"),
        ("miri", "miri"),
        ("rust-analyzer", "rust-analyzer"),
        ("rust-bindgen", "bindgen"),
    ]);

    let out = fs::File::create("cache/prs")?;
    let mut out = io::BufWriter::new(out);
    for pr in prs {
        let url = pr.html_url;
        if let Some(unprefixed) = url.path().strip_prefix("/rust-lang/") {
            if let Some(repo) = unprefixed.split_once("/") {
                if let Some(reponame) = repos.get(repo.0) {
                    if !pr.title.starts_with(reponame) {
                        writeln!(
                            out,
                            "* [{}: {}]({})",
                            reponame,
                            pr.title.trim_matches(&[' ', '.'][..]),
                            url
                        )?;
                        continue;
                    }
                }
            }
        }
        writeln!(
            out,
            "* [{}]({})",
            pr.title.trim_matches(&[' ', '.'][..]),
            url
        )?;
    }
    Ok(total_count)
}

fn prev() -> Result<HashSet<String>, Box<dyn Error>> {
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
    Ok(previous)
}

fn filter(config: &Config) -> Result<Vec<String>, Box<dyn Error>> {
    let previous = prev()?;
    let prs = fs::File::open("cache/prs")?;
    let mut sorted_prs = Vec::new();
    let ignore_list = get_list(config, "ignore")?;
    let order = get_list(config, "order")?;
    let code_words: HashSet<String> = get_list(config, "code_keywords")?
        .into_iter()
        .cloned()
        .collect();
    for pr in BufReader::new(prs).lines() {
        let pr = pr?;
        let (title, href) = pr.rsplit_once("](").unwrap_or((&pr, ""));
        let title = format_title(code_words, title.strip_prefix("* [").unwrap_or(title));
        if previous.contains(href) {
            continue;
        }
        let lower = pr.to_lowercase();
        if !ignore_list.iter().any(|kw| lower.contains(kw)) {
            sorted_prs.push(format!("* [{title}]({href}",));
        }
    }
    let ord = &order[..];
    sorted_prs.sort_by(|a, b| repo_title(a, ord).cmp(&repo_title(b, ord)));
    let mut filtered_prs = fs::File::create("cache/filteredprs")?;
    for pr in &sorted_prs {
        writeln!(filtered_prs, "{}", pr)?;
    }
    Ok(sorted_prs)
}

fn file_path() -> Result<PathBuf, Box<dyn Error>> {
    std::fs::read_dir("../this-week-in-rust/draft")?
        .filter_map(|e| e.ok())
        .map(|f| f.path())
        .find(|p| {
            p.extension()
                .map_or(false, |e| e.eq_ignore_ascii_case("md"))
        })
        .ok_or_else(|| Cow::Borrowed("Draft not found").into())
}

fn get_number(contents: &str) -> Result<&str, Box<dyn Error>> {
    if let Some(n) = contents
        .split("Number: ")
        .nth(1)
        .and_then(|n| n.split('\n').next())
    {
        Ok(n)
    } else {
        Err("Number not found in draft".into())
    }
}

fn command(binary: &str, args: &[&str], cwd: &str) -> Result<String, Box<dyn Error>> {
    println!("Running {} {}", binary, args.join(" "));
    let mut cmd = Command::new(binary);
    cmd.args(args);
    if cwd != "" {
        cmd.current_dir(cwd);
    }
    let out = cmd.output()?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    env::set_current_dir(WORKSPACE)?;
    let cmd = env::args().nth(1);
    match cmd.as_ref().map_or("", |s| s) {
        "week" => {
            let week_spec = week()?;
            fs::write("cache/week_spec", &week_spec)?;
            println!(
                "https://github.com/search?q=is%3Apr+org%3Arust-lang+is%3Amerged+merged%3A{}",
                week_spec
            );
        }
        "token" => {
            println!("[{}]", token()?);
        }
        "prs" => {
            prs(&fs::read_to_string("cache/week_spec")?).await?;
        }
        "filter" => {
            filter(&read_config()?)?;
        }
        "branches" => {
            let branches_out = command("git", &["branch"], TWIR)?;
            let mut current = "";
            let branches = branches_out
                .lines()
                .map(|l| {
                    if let Some(c) = l.strip_prefix("* ") {
                        current = c;
                        c
                    } else {
                        l.trim()
                    }
                })
                .collect::<Vec<_>>();
            println!("{}\n* {}", branches.join(", "), current);
        }
        "editor" => {
            let conf = read_config()?;
            let editor = conf
                .get("editor")
                .ok_or(Cow::Borrowed("needs `editor=<path>` in config"))?;
            command(editor, &[], "")?;
        }
        "browser" => {
            let conf = read_config()?;
            let firefox = conf
                .get("firefox")
                .ok_or(Cow::Borrowed("needs `firefox=<path>` in config"))?;
            //command(firefox, &[], "")?;
            println!("{firefox:?}");
        }
        "start" => {
            println!("start");
            let conf = read_config()?;
            let firefox = conf
                .get("firefox")
                .ok_or(Cow::Borrowed("needs `firefox=<path>` in config"))?;
            let editor = conf
                .get("editor")
                .ok_or(Cow::Borrowed("needs `editor=<path>` in config"))?;
            command("git", &["checkout", "master"], TWIR)?;
            command("git", &["pull"], TWIR)?;
            let file_path = file_path()?;
            let contents = std::fs::read_to_string(&file_path)?;
            if !(contents.contains("<!-- COTW goes here -->")
                && contents.contains("<!-- QOTW goes here -->")
                && contents.contains("<!-- Rust updates go here -->"))
            {
                println!("error: setup not done yet. Try again later.");
                return Ok(());
            }
            command(
                firefox,
                &[
                    "--new-tab",
                    "https://users.rust-lang.org/t/crate-of-the-week/2704/last",
                    "--new-tab",
                    "https://users.rust-lang.org/t/twir-quote-of-the-week/328/last",
                ],
                "",
            )?;
            // insert the C/QotW templates & filtered PRs into the document
            let week_spec = std::fs::read_to_string("cache/week_spec")?;
            let num_prs = prs(&week_spec).await?;
            println!("found {} prs", num_prs);
            let filtered_prs = filter(&conf)?.join("\n");
            println!("filtered prs");
            let updates = format!(
                "{num_prs} pull requests were [merged in the last week][merged]\n\
\n[merged]: https://github.com/search?q=is%3Apr+org%3Arust-lang+is%3Amerged+merged%3A{week_spec}\n\
\n{filtered_prs}"
            );
            let contents = contents
                .replace(
                    "<!-- COTW goes here -->",
                    "This week's crate is [](), a \n\nThanks to []() for the suggestion!",
                )
                .replace(
                    "<!-- QOTW goes here -->",
                    "> \n\n– []()\n\nThanks to []() for the suggestion!",
                )
                .replace("<!-- Rust updates go here -->", &updates);
            // overwrite with out changes
            std::fs::write(&file_path, contents)?;
            println!("updated contents, opening editor");
            // open the document with editor
            command(editor, &[file_path.as_os_str().to_str().unwrap()], "")?;
        }
        "check" => {
            let file_path = file_path()?;
            let contents = std::fs::read_to_string(&file_path)?;
            // check markdown
            let chapters = contents.split("\n##");
            let mut err = 0;
            for chapter in chapters {
                let (title, text) = chapter.split_once("\n").unwrap();
                match title.trim() {
                    "Crate of the Week" | "Quote of the Week" => {
                        check_markdown(chapter);
                    }
                    "Updates from the Rust Project" => {
                        err += check_markdown(chapter);
                        let mut parts = text.splitn(3, "\n\n");
                        let num = parts.next().expect("missing Updates prs num");
                        assert!(
                            num.ends_with(" pull requests were [merged in the last week][merged]")
                        );
                        let link = parts.next().expect("missing Updates link");
                        assert!(link.starts_with("[merged]: https://github.com/search?q=is%3Apr+org%3Arust-lang+is%3Amerged+merged%3A"));
                        let prs = parts.next().expect("missing PRs");
                        for pr in prs.lines() {
                            if let Some(p) = pr.strip_prefix("* [") {
                                if let Some(p) = p.strip_suffix(")") {
                                    if let Some((title, link)) = p.split_once("](") {
                                        err += check_title(title);
                                        err += check_link(link)
                                    } else {
                                        println!("Wrong PR link: {}", pr);
                                        err += 1;
                                    }
                                } else {
                                    println!("Wrong PR link: {}", pr);
                                    err += 1;
                                }
                            } else {
                                println!("Wrong PR link: {}", pr);
                                err += 1;
                            }
                        }
                    }
                    _ => {}
                }
                if err > 0 {
                    if err == 1 {
                        panic!("There was 1 error");
                    }
                    panic!("There were {} errors", err);
                }
            }
        }
        "push" => {
            let conf = read_config()?;
            let firefox = conf
                .get("firefox")
                .ok_or(Cow::Borrowed("needs `firefox=<path>` in config"))?;
            let file_path = file_path()?;
            let contents = std::fs::read_to_string(&file_path)?;
            let number = get_number(&contents)?;
            // create, commit & push the new branch
            command(
                "git",
                &["checkout", "-b", &format!("twir-{}", number)],
                TWIR,
            )?;
            command(
                "git",
                &[
                    "add",
                    &format!("draft/{}", file_path.file_name().unwrap().to_string_lossy()),
                ],
                TWIR,
            )?;
            command("git", &["commit", "-m", "C/QotW and notable changes"], TWIR)?;
            command("git", &["push", "llogiq"], TWIR)?;
            // open the PR view
            command(
                firefox,
                &[
                    "--new-tab",
                    &format!(
                        "https://github.com/llogiq/this-week-in-rust/pull/new/twir-{}",
                        number
                    ),
                ],
                "",
            )?;
            // update the week spec
            let week_spec = week()?;
            println!("set week to {}", week_spec);
            fs::write("cache/week_spec", &week_spec)?;
            // move cache/prs to cache/last_prs
            std::fs::rename("cache/prs", "cache/last_prs")?;
            // delete previous branch
            if let Ok(num) = str::parse::<u64>(number) {
                let previous_branch = format!("twir-{}", num - 1);
                command("git", &["branch", "-d", &previous_branch], TWIR)?;
                command(
                    "git",
                    &["push", "--delete", "llogiq", &previous_branch],
                    TWIR,
                )?;
            }
        }
        _ => {
            println!("usage: twirer [prs <spec>|filter]");
        }
    }
    Ok(())
}

fn check_link(link: &str) -> usize {
    if let Some(rest) = link.strip_prefix("https://github.com/rust-lang/") {
        let mut parts = rest.splitn(3, '/');
        let repo = parts.next().unwrap_or("?");
        let pull = parts.next().unwrap_or("?");
        let number = parts.next().unwrap_or("?");
        if !repo
            .chars()
            .all(|c| ['_', '-', '.'].contains(&c) || c.is_ascii_alphanumeric())
            || pull != "pull"
            || str::parse::<u32>(number).is_err()
        {
            println!("wong link: {}", link);
            return 1;
        }
    }
    0
}

fn check_title(title: &str) -> usize {
    let mut in_code = false;
    let mut escape = false;
    for c in title.chars() {
        if c == '`' {
            in_code = !in_code;
        } else if !in_code {
            if !escape && ['<', '>', '[', ']', '_'].contains(&c) {
                println!("unescaped `{}` in non-code title: {}", c, title);
                return 1;
            }
            escape = c == '\\';
        }
    }
    if escape {
        println!("wonky backslash at the end: {}", title);
        1
    } else if in_code {
        println!("unmatched backticks in {}", title);
        1
    } else {
        0
    }
}

fn has_unescaped(mut haystack: &str, needle: &str) -> bool {
    while let Some(pos) = haystack.find(needle) {
        if haystack.as_bytes()[pos.saturating_sub(1)] == b'\\' {
            haystack = &haystack[pos + needle.len()..];
        } else {
            return true;
        }
    }
    false
}

#[derive(Debug)]
struct Word<'t> {
    text: &'t str,
    is_code: bool,
    colon: bool,
}

impl<'t> Word<'t> {
    fn new(s: &'t str, in_code: bool, code_words: &HashSet<String>) -> (Self, bool, bool) {
        let (text, colon) = if let Some(t) = s.strip_suffix(':') {
            (t, true)
        } else {
            (s, false)
        };
        let (text, out_code) = if let Some(t) = text.strip_suffix('`') {
            (t, true)
        } else {
            (text, false)
        };
        let (text, colon) = if let Some(t) = text.strip_suffix(':') {
            (t, true)
        } else {
            (text, colon)
        };
        let (text, into_code) = if let Some(t) = text.strip_prefix('`') {
            (t, true)
        } else {
            (text, false)
        };
        let is_code = into_code ||
            in_code
            // snake case terms
            || has_unescaped(text, "_")
            // generics
            || has_unescaped(text, "<")
            // paths
            || text.contains("::")
            // attributes
            || text.contains("#[")
            // function calls, but not parenthesized texts
            || text.contains("(") && text.ends_with(")") && !text.starts_with('(')
            || code_words.contains(text);
        (
            Word {
                text,
                is_code,
                colon,
            },
            into_code,
            out_code,
        )
    }
}

fn format_title(code_words: &HashSet<String>, title: &str) -> String {
    let mut in_code = false;
    let mut words = Vec::new();
    for text in title.trim().split_whitespace() {
        let (word, into_code, out_code) = Word::new(text, in_code, code_words);
        in_code = (in_code | into_code) & !out_code;
        words.push(word);
    }
    let mut first = true;
    let mut result = String::new();
    for w in 0..words.len() {
        let word = &words[w];
        if word.is_code && w.checked_sub(1).map_or(true, |i| !words[i].is_code) {
            result.push('`');
            result.push_str(word.text);
            first = false;
        } else if first
            && word.text.starts_with(char::is_uppercase)
            && word.text.chars().any(char::is_lowercase)
        {
            // lowercase initial title case
            let mut c = word.text.chars();
            result.extend(c.next().unwrap().to_lowercase());
            result.push_str(c.as_str());
            first &= word.colon;
        } else {
            result.push_str(word.text);
            first &= word.colon;
        }
        // close any opened code span
        if word.is_code && words.get(w + 1).map_or(true, |n| !n.is_code) {
            result.push('`');
        }
        // add any given colon
        if word.colon {
            result.push(':');
        }
        // add whitespace between words
        if w < words.len() - 1 {
            result.push(' ');
        }
    }
    result
}

fn check_markdown(chapter: &str) -> usize {
    let mut err = 0;
    for l in chapter.split('\n') {
        // check for stray whitespace
        if let 0 | 2 = l
            .chars()
            .rev()
            .take(3)
            .filter(|&c| c.is_whitespace())
            .count()
        {
            // Ok
        } else {
            println!("line: `{}` ends with irregular whitespace", l);
            err += 1;
        }
        // TODO: check link titles, uneven code spans, etc.
    }
    err
}
