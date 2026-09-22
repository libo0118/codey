//! Conservative input-aware classification; this never grants a capability.
use super::rules::{ToolClass, classify_tool, normalize_tool_name};
use serde_json::Value;

pub(crate) fn classify(tool: &str, input: Option<&Value>) -> ToolClass {
    let normalized = normalize_tool_name(tool);
    let resource = normalized
        .strip_prefix("functions.")
        .or_else(|| normalized.strip_prefix("functions__"))
        .unwrap_or(&normalized);
    if matches!(
        resource,
        "read_mcp_resource" | "list_mcp_resources" | "list_mcp_resource_templates"
    ) {
        return ToolClass::Read;
    }
    let class = classify_tool(tool);
    if normalized == "exec" {
        return literal_call(input).unwrap_or(class);
    }
    if matches!(
        normalized.as_str(),
        "exec_command" | "bash" | "shell" | "powershell"
    ) {
        return command(input).unwrap_or(class);
    }
    class
}

// Deliberately a complete, single-call grammar, not a JavaScript substring search.
// ponytail: only JSON literals; add an audited AST parser if general JS is needed.
fn literal_call(input: Option<&Value>) -> Option<ToolClass> {
    let input = input?;
    let source = if let Some(source) = input.as_str() {
        source
    } else {
        let object = input.as_object()?;
        if object.len() != 1 {
            return None;
        }
        object
            .get("code")
            .or_else(|| object.get("input"))?
            .as_str()?
    };
    let call = source
        .trim()
        .strip_prefix("text(await tools.")?
        .strip_suffix(");")?;
    let (name, args) = call.split_once('(')?;
    if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return None;
    }
    let args: Value = serde_json::from_str(args.strip_suffix(')')?).ok()?;
    // No recursion, unknown MCP commands, mutation, or additional JS statements.
    if !matches!(
        name,
        "exec_command"
            | "web__run"
            | "read_mcp_resource"
            | "list_mcp_resources"
            | "list_mcp_resource_templates"
    ) {
        return None;
    }
    let class = classify(name, Some(&args));
    matches!(class, ToolClass::Read | ToolClass::Network).then_some(class)
}

fn command(input: Option<&Value>) -> Option<ToolClass> {
    let object = input?.as_object()?;
    for (key, value) in object {
        let valid = match key.as_str() {
            "cmd" | "command" | "workdir" | "cwd" => value.is_string(),
            "max_output_tokens" | "yield_time_ms" | "timeout_ms" | "timeout" => {
                value.as_u64().is_some()
            }
            "tty" | "login" => value == &Value::Bool(false),
            "sandbox_permissions" => value.as_str() == Some("use_default"),
            _ => false,
        };
        if !valid {
            return None;
        }
    }
    if object.contains_key("cmd") && object.contains_key("command") {
        return None;
    }
    let words = words(
        object
            .get("cmd")
            .or_else(|| object.get("command"))?
            .as_str()?,
    )?;
    match words.first()?.as_str() {
        "git" | "git.exe" if git(&words[1..]) => Some(ToolClass::Read),
        "curl" | "curl.exe" if curl(&words[1..]) => Some(ToolClass::Network),
        "gh" | "gh.exe" if gh(&words[1..]) => Some(ToolClass::Network),
        _ => None,
    }
}

// Common subset of PowerShell and POSIX tokenization. No escapes, interpolation,
// operators, wildcards or executable paths; quotes may only surround whole words.
fn words(source: &str) -> Option<Vec<String>> {
    let mut result = Vec::new();
    let mut chars = source.chars().peekable();
    while chars.peek().is_some() {
        while chars.peek() == Some(&' ') {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }
        let quote = match chars.peek() {
            Some('\'') | Some('"') => chars.next(),
            _ => None,
        };
        let mut word = String::new();
        let mut closed = quote.is_none();
        for c in chars.by_ref() {
            if Some(c) == quote {
                closed = true;
                break;
            }
            if c == ' ' && quote.is_none() {
                break;
            }
            if !(c.is_alphanumeric() || "-._/:=+".contains(c) || (c == ' ' && quote.is_some())) {
                return None;
            }
            word.push(c);
        }
        if !closed
            || word.is_empty()
            || (quote.is_some() && chars.peek().is_some_and(|c| *c != ' '))
        {
            return None;
        }
        result.push(word);
    }
    Some(result)
}

fn git(args: &[String]) -> bool {
    let mut i = 0;
    let (mut pager, mut locks, mut monitor, mut no_fetch) = (false, false, false, false);
    let mut signature = false;
    while let Some(arg) = args.get(i) {
        match arg.as_str() {
            "--no-pager" => pager = true,
            "--no-optional-locks" => locks = true,
            "--no-lazy-fetch" => no_fetch = true,
            "-C" => {
                i += 1;
                if !args.get(i).is_some_and(|s| !s.starts_with('-')) {
                    return false;
                }
            }
            "-c" => {
                i += 1;
                match args.get(i).map(String::as_str) {
                    Some("core.fsmonitor=false") => monitor = true,
                    Some("log.showSignature=false") => signature = true,
                    Some(s) if s.starts_with("safe.directory=") && s.len() > 15 => {}
                    _ => return false,
                }
            }
            _ => break,
        }
        i += 1;
    }
    if !(pager && locks && monitor && no_fetch) {
        return false;
    }
    let Some(subcommand) = args.get(i).map(String::as_str) else {
        return false;
    };
    let tail = &args[i + 1..];
    if !matches!(
        subcommand,
        "log" | "diff" | "show" | "ls-files" | "ls-tree" | "rev-parse" | "merge-base"
    ) {
        return false;
    }
    // Flags after -- are paths, never evidence of a safety option.
    let options = &tail[..tail.iter().position(|s| s == "--").unwrap_or(tail.len())];
    let has = |flag: &str| options.iter().any(|s| s == flag);
    if matches!(subcommand, "diff" | "show")
        && !(has("--no-ext-diff") && has("--no-textconv") && has("--ignore-submodules=all"))
    {
        return false;
    }
    if matches!(subcommand, "log" | "show") && !(signature || has("--no-show-signature")) {
        return false;
    }
    if matches!(subcommand, "log" | "show") && !(has("--oneline") || has("--format=oneline")) {
        return false;
    }
    // Worktree status/diff can execute repository clean filters even with
    // --no-ext-diff and --no-textconv. Only index/object reads are proved here.
    if subcommand == "diff" && !(has("--cached") || has("--staged")) {
        return false;
    }
    let mut paths = false;
    let mut i = 0;
    while let Some(arg) = tail.get(i) {
        if arg == "--" {
            paths = true;
            i += 1;
            continue;
        }
        if !paths && arg.starts_with('-') {
            let allowed = match subcommand {
                "log" => matches!(
                    arg.as_str(),
                    "--oneline"
                        | "--format=oneline"
                        | "--all"
                        | "--no-show-signature"
                        | "--no-decorate"
                ),
                "diff" | "show" => matches!(
                    arg.as_str(),
                    "--oneline"
                        | "--format=oneline"
                        | "--no-ext-diff"
                        | "--no-textconv"
                        | "--ignore-submodules=all"
                        | "--no-show-signature"
                        | "--stat"
                        | "--name-only"
                        | "--name-status"
                        | "--shortstat"
                        | "--numstat"
                        | "--cached"
                        | "--staged"
                        | "--no-color"
                ),
                "ls-files" => matches!(
                    arg.as_str(),
                    "--cached" | "--stage" | "--others" | "--exclude-standard"
                ),
                "ls-tree" => matches!(arg.as_str(), "-r" | "--name-only" | "--long"),
                "rev-parse" => matches!(
                    arg.as_str(),
                    "--show-toplevel"
                        | "--git-dir"
                        | "--is-inside-work-tree"
                        | "--verify"
                        | "--short"
                ),
                "merge-base" => matches!(arg.as_str(), "--all" | "--is-ancestor" | "--octopus"),
                _ => false,
            };
            if subcommand == "log" && arg == "-n" {
                i += 1;
                if !tail.get(i).is_some_and(|s| s.parse::<u32>().is_ok()) {
                    return false;
                }
            } else if !allowed {
                return false;
            }
        }
        i += 1;
    }
    true
}

fn curl(args: &[String]) -> bool {
    if args.first().map(String::as_str) != Some("-q") {
        return false;
    }
    let mut url = false;
    let mut i = 1;
    while let Some(arg) = args.get(i) {
        match arg.as_str() {
            "--head" | "-I" | "--silent" | "-s" | "--show-error" | "-S" | "--fail" | "-f" => {}
            "--request" | "-X" => {
                i += 1;
                if !matches!(args.get(i).map(String::as_str), Some("GET" | "HEAD")) {
                    return false;
                }
            }
            s if (s.starts_with("https://") || s.starts_with("http://")) && !url => url = true,
            _ => return false,
        }
        i += 1;
    }
    url
}

fn gh(args: &[String]) -> bool {
    // Explicit method, GitHub-relative endpoint, no body, output or host overrides.
    args.len() == 4
        && args[0] == "api"
        && args[1] == "--method"
        && matches!(args[2].as_str(), "GET" | "HEAD")
        && args[3].starts_with(|c: char| c.is_ascii_alphabetic())
        && !args[3].contains(':')
        && !args[3].contains("..")
        && args[3].contains('/')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn read_only_commands_and_literal_wrappers() {
        for cmd in [
            "git --no-pager --no-optional-locks --no-lazy-fetch -c core.fsmonitor=false ls-files",
            "git --no-pager --no-optional-locks --no-lazy-fetch -c core.fsmonitor=false -C C:/repo log --oneline --no-show-signature -n 5",
            "git.exe --no-pager --no-optional-locks --no-lazy-fetch -c core.fsmonitor=false -c safe.directory=C:/项目 ls-files",
            "git --no-pager --no-optional-locks --no-lazy-fetch -c core.fsmonitor=false diff --cached --no-ext-diff --no-textconv --ignore-submodules=all --stat",
            "curl.exe -q --head https://example.com",
            "curl -q https://example.com",
            "gh api --method GET repos/owner/repo",
        ] {
            assert!(
                matches!(
                    classify("exec_command", Some(&json!({"cmd":cmd}))),
                    ToolClass::Read | ToolClass::Network
                ),
                "{cmd}"
            );
        }
        for tool in [
            "read_mcp_resource",
            "functions.list_mcp_resources",
            "functions__list_mcp_resource_templates",
        ] {
            assert_eq!(classify(tool, Some(&json!({}))), ToolClass::Read);
        }
        for code in [
            r#"text(await tools.exec_command({"cmd":"git --no-pager --no-optional-locks --no-lazy-fetch -c core.fsmonitor=false ls-files"}));"#,
            r#"text(await tools.web__run({"search_query":[{"q":"rust"}]}));"#,
            r#"text(await tools.read_mcp_resource({"server":"a","uri":"b"}));"#,
        ] {
            assert!(
                matches!(
                    classify("functions.exec", Some(&json!(code))),
                    ToolClass::Read | ToolClass::Network
                ),
                "{code}"
            );
            assert!(matches!(
                classify("functions.exec", Some(&json!({"code":code}))),
                ToolClass::Read | ToolClass::Network
            ));
            assert!(matches!(
                classify("functions.exec", Some(&json!({"input":code}))),
                ToolClass::Read | ToolClass::Network
            ));
        }
    }

    #[test]
    fn unproved_commands_and_javascript_stay_commands() {
        assert_eq!(
            classify(
                "exec_command",
                Some(
                    &json!({"cmd":"git --no-pager --no-optional-locks -c core.fsmonitor=false ls-files"})
                )
            ),
            ToolClass::Command
        );
        for suffix in [
            "status --short",
            "diff --no-ext-diff --no-textconv --ignore-submodules=all",
            "diff --cached -- --no-ext-diff --no-textconv --ignore-submodules=all",
            "log -- --oneline --no-show-signature",
            "ls-files @args",
            "ls-files a,b",
            "ls-files; evil",
            "ls-files --output=out",
            "ls-remote https://github.com/owner/repo",
        ] {
            let cmd = format!(
                "git --no-pager --no-optional-locks --no-lazy-fetch -c core.fsmonitor=false {suffix}"
            );
            assert_eq!(
                classify("exec_command", Some(&json!({"cmd":cmd}))),
                ToolClass::Command,
                "{suffix}"
            );
        }
        for extra in [
            json!({"shell":"powershell"}),
            json!({"env":{"PATH":"evil"}}),
            json!({"login":true}),
            json!({"sandbox_permissions":"require_escalated"}),
        ] {
            let mut input = json!({"cmd":"git --no-pager --no-optional-locks --no-lazy-fetch -c core.fsmonitor=false ls-files"});
            input
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            assert_eq!(classify("exec_command", Some(&input)), ToolClass::Command);
        }
        for suffix in [
            "commit -m x",
            "push",
            "fetch",
            "-c core.fsmonitor=evil status",
            "-c core.pager=evil log --oneline --no-show-signature",
            "diff --no-ext-diff --no-textconv --output=out",
            "diff --no-ext-diff --no-textconv --ext-diff",
            "log --oneline --no-show-signature --show-signature",
            "log --oneline --no-show-signature --format=bad",
            "status; evil",
            "status | cat",
            "status > out",
            "status $(evil)",
            "ls-remote origin",
        ] {
            let cmd = format!(
                "git --no-pager --no-optional-locks --no-lazy-fetch -c core.fsmonitor=false {suffix}"
            );
            assert_eq!(
                classify("exec_command", Some(&json!({"cmd":cmd}))),
                ToolClass::Command,
                "{suffix}"
            );
        }
        for cmd in [
            "git commit -m x",
            "git push",
            "git fetch",
            "git -c core.fsmonitor=evil status",
            "git diff --ext-diff",
            "git log --show-signature",
            "git status; whoami",
            "git status | cat",
            "git status > out",
            "git status $(evil)",
            "curl -X POST https://example.com",
            "curl -o out https://example.com",
            "curl https://example.com",
            "python run.py",
            "gh api -f x=y repos/owner/repo",
        ] {
            assert_eq!(
                classify("exec_command", Some(&json!({"cmd":cmd}))),
                ToolClass::Command,
                "{cmd}"
            );
        }
        for input in [
            json!({"cmd":"git status","command":"evil"}),
            json!({"cmd":"git status","shell":"evil"}),
            json!({"cmd":"git status","env":{"PATH":"evil"}}),
            json!({"cmd":"git status","sandbox_permissions":"require_escalated"}),
        ] {
            assert_eq!(classify("exec_command", Some(&input)), ToolClass::Command);
        }
        for code in [
            r#"text(await tools.exec_command({"cmd":"git status"})); evil();"#,
            r#"text(await tools.exec_command({"cmd":"git status"}));text(await tools.apply_patch("x"));"#,
            r#"text(await tools.exec_command({"cmd":"git "+"status"}));"#,
            r#"text(await tools.exec_command({cmd:"git status"}));"#,
            r#"text(await tools["exec_command"]({"cmd":"git status"}));"#,
        ] {
            assert_eq!(
                classify("functions.exec", Some(&json!(code))),
                ToolClass::Command
            );
        }
    }
}
