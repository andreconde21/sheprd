//! andreconde fork (sheprd): `sheprd card …`, the card ledger CLI (SHE-100008). An agent reports
//! per card what it is doing, asks the user questions with suggested answers and records its
//! decisions; the right work panel shows them. The pane defaults to the calling one
//! (`HERDR_PANE_ID`); `--pane` targets another, and is required with `--machine`.

use crate::api::schema::{
    LedgerAskParams, LedgerCompleteParams, LedgerDecideParams, LedgerItemTarget, LedgerListParams,
    LedgerQuestionInput, LedgerUpdateParams, Method, Request,
};

const USAGE: &str = "usage: sheprd card <update|ask|decide|done|remove|list> …";

pub(super) fn run_card_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(String::as_str) else {
        print_card_help();
        return Ok(2);
    };
    let parsed = match subcommand {
        "update" | "ask" | "decide" | "done" | "remove" | "list" => {
            parse_card_args(subcommand, &args[1..])
        }
        "help" | "--help" | "-h" => {
            print_card_help();
            return Ok(0);
        }
        _ => {
            print_card_help();
            return Ok(2);
        }
    };
    let command = match parsed {
        Ok(command) => command,
        Err(message) => {
            eprintln!("{message}");
            return Ok(2);
        }
    };
    let json = command.json;
    let response = super::send_request(&Request {
        id: format!("cli:card:{subcommand}"),
        method: command.method,
    })?;
    if let Some(error) = response.get("error") {
        if no_ledger_on_server(error) {
            eprintln!(
                "this server has no card ledger; update sheprd where the pane runs (`sheprd update`) and restart its server"
            );
        } else {
            eprintln!(
                "{}",
                serde_json::to_string(&response).map_err(std::io::Error::other)?
            );
        }
        return Ok(1);
    }
    let result = &response["result"];
    if json {
        println!(
            "{}",
            serde_json::to_string(result).map_err(std::io::Error::other)?
        );
        return Ok(0);
    }
    match subcommand {
        "ask" => println!("{}", result["question_id"]),
        "list" => print!("{}", format_list(result)),
        _ => {}
    }
    Ok(0)
}

/// An older server rejects the `ledger.*` methods as an invalid request.
fn no_ledger_on_server(error: &serde_json::Value) -> bool {
    matches!(
        error["code"].as_str(),
        Some("invalid_request" | "unknown_method")
    ) && error["message"]
        .as_str()
        .is_some_and(|message| message.contains("ledger."))
}

#[derive(Debug, PartialEq)]
struct CardCommand {
    method: Method,
    json: bool,
}

fn value<'a>(args: &'a [String], index: usize, flag: &str) -> Result<&'a String, String> {
    args.get(index + 1)
        .ok_or_else(|| format!("missing value for {flag}"))
}

fn parse_card_args(subcommand: &str, args: &[String]) -> Result<CardCommand, String> {
    let mut positional = Vec::new();
    let mut pane = None;
    let mut json = false;
    let mut title = None;
    let mut status = None;
    let mut summary = None;
    let mut link = None;
    let mut inferred = false;
    let mut inferred_questions = Vec::new();
    let mut clear_questions = false;
    let mut options = Vec::new();
    let mut recommend = None;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        let mut step = 2;
        match arg {
            "--pane" => pane = Some(value(args, index, arg)?.clone()),
            "--json" => {
                json = true;
                step = 1;
            }
            "--title" if subcommand == "update" => title = Some(value(args, index, arg)?.clone()),
            "--status" if subcommand == "update" => status = Some(value(args, index, arg)?.clone()),
            "--summary" if matches!(subcommand, "update" | "done") => {
                summary = Some(value(args, index, arg)?.clone())
            }
            "--link" if subcommand == "update" => link = Some(value(args, index, arg)?.clone()),
            "--inferred" if subcommand == "update" => {
                inferred = true;
                step = 1;
            }
            "--inferred-question" if subcommand == "update" => {
                inferred_questions.push(value(args, index, arg)?.clone())
            }
            "--clear-questions" if subcommand == "update" => {
                clear_questions = true;
                step = 1;
            }
            "--option" if subcommand == "ask" => options.push(value(args, index, arg)?.clone()),
            "--recommend" if subcommand == "ask" => {
                let raw = value(args, index, arg)?;
                let number = raw
                    .parse::<usize>()
                    .ok()
                    .filter(|number| *number >= 1)
                    .ok_or_else(|| format!("--recommend takes an option number from 1: {raw}"))?;
                recommend = Some(number - 1);
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown option for `card {subcommand}`: {other}"));
            }
            _ => {
                positional.push(args[index].clone());
                step = 1;
            }
        }
        index += step;
    }

    let wanted = match subcommand {
        "ask" | "decide" => 2,
        "list" => 0,
        _ => 1,
    };
    if positional.len() != wanted {
        return Err(match subcommand {
            "update" => "usage: sheprd card update <ID> [--title T] [--status S] [--summary S] [--link URL] [--pane P]".into(),
            "ask" => "usage: sheprd card ask <ID> \"question?\" [--option A]… [--recommend N] [--pane P]".into(),
            "decide" => "usage: sheprd card decide <ID> \"decision\" [--pane P]".into(),
            "done" => "usage: sheprd card done <ID> [--summary S] [--pane P]".into(),
            "remove" => "usage: sheprd card remove <ID> [--pane P]".into(),
            "list" => "usage: sheprd card list [--pane P] [--json]".into(),
            _ => USAGE.into(),
        });
    }
    if (!inferred_questions.is_empty() || clear_questions) && !inferred {
        return Err("--inferred-question and --clear-questions need --inferred".into());
    }
    if recommend.is_some_and(|index| index >= options.len()) {
        return Err("--recommend must name one of the --option values".into());
    }

    let pane = pane.or_else(super::target::caller_pane_id);
    if subcommand == "list" {
        return Ok(CardCommand {
            method: Method::LedgerList(LedgerListParams { pane_id: pane }),
            json,
        });
    }
    let Some(pane_id) = pane else {
        return Err(if super::target::is_remote() {
            "--pane is required with --machine".into()
        } else {
            "not inside a pane: pass --pane <PANE_ID>".into()
        });
    };
    let mut positional = positional.into_iter();
    let item_id = positional.next().unwrap_or_default();
    let method = match subcommand {
        "update" => Method::LedgerUpdate(LedgerUpdateParams {
            pane_id,
            item_id,
            title,
            status,
            summary,
            link,
            inferred,
            inferred_questions: (inferred && (clear_questions || !inferred_questions.is_empty()))
                .then(|| {
                    inferred_questions
                        .into_iter()
                        .map(|text| LedgerQuestionInput {
                            text,
                            ..LedgerQuestionInput::default()
                        })
                        .collect()
                }),
        }),
        "ask" => Method::LedgerAsk(LedgerAskParams {
            pane_id,
            item_id,
            text: positional.next().unwrap_or_default(),
            options,
            recommended: recommend,
        }),
        "decide" => Method::LedgerDecide(LedgerDecideParams {
            pane_id,
            item_id,
            text: positional.next().unwrap_or_default(),
        }),
        "done" => Method::LedgerComplete(LedgerCompleteParams {
            pane_id,
            item_id,
            summary,
        }),
        _ => Method::LedgerRemove(LedgerItemTarget { pane_id, item_id }),
    };
    Ok(CardCommand { method, json })
}

/// A field as printed: control characters become spaces, so whatever a server sends cannot move
/// the cursor or recolour the terminal.
fn text(value: &serde_json::Value) -> Option<String> {
    value.as_str().map(|value| {
        value
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect()
    })
}

/// `card list` as text: per pane, one line per card, then its summary, questions and decisions.
fn format_list(result: &serde_json::Value) -> String {
    let mut out = String::new();
    for pane in result["panes"].as_array().into_iter().flatten() {
        out.push_str(&text(&pane["pane_id"]).unwrap_or_else(|| "?".into()));
        out.push('\n');
        let items = pane["items"].as_array().cloned().unwrap_or_default();
        if items.is_empty() {
            out.push_str("  (no cards)\n");
        }
        for item in &items {
            let questions = item["questions"].as_array().cloned().unwrap_or_default();
            let mut line = format!("  {}", text(&item["id"]).unwrap_or_else(|| "?".into()));
            if let Some(title) = text(&item["title"]) {
                line.push_str(&format!("  {title}"));
            }
            if let Some(status) = text(&item["status"]) {
                line.push_str(&format!("  [{status}]"));
            }
            if item["done"].as_bool() == Some(true) {
                line.push_str("  done");
            }
            if !questions.is_empty() {
                line.push_str(&format!("  ?{}", questions.len()));
            }
            if item["inferred"].as_bool() == Some(true) {
                line.push_str("  (inferred)");
            }
            out.push_str(&line);
            out.push('\n');
            if let Some(summary) = text(&item["summary"]) {
                out.push_str(&format!("    {summary}\n"));
            }
            for question in &questions {
                let mut line = format!(
                    "    ? [{}] {}",
                    question["id"],
                    text(&question["text"]).unwrap_or_default()
                );
                let recommended = question["recommended"].as_u64();
                for (index, option) in question["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    let star = if recommended == Some(index as u64) {
                        " ★"
                    } else {
                        ""
                    };
                    line.push_str(&format!(
                        "  {}) {}{star}",
                        index + 1,
                        text(option).unwrap_or_default()
                    ));
                }
                out.push_str(&line);
                out.push('\n');
            }
            for decision in item["decisions"].as_array().into_iter().flatten() {
                out.push_str(&format!(
                    "    ✓ {}\n",
                    text(&decision["text"]).unwrap_or_default()
                ));
            }
        }
    }
    if out.is_empty() {
        out.push_str("no cards\n");
    }
    out
}

fn print_card_help() {
    eprintln!("sheprd card commands (the card ledger the work panel shows):");
    eprintln!("  sheprd card update <ID> [--title T] [--status S] [--summary S] [--link URL]");
    eprintln!("  sheprd card ask <ID> \"question?\" [--option A]… [--recommend N]");
    eprintln!("  sheprd card decide <ID> \"decision\"");
    eprintln!("  sheprd card done <ID> [--summary S]");
    eprintln!("  sheprd card remove <ID>");
    eprintln!("  sheprd card list [--json]");
    eprintln!(
        "The pane is the calling one; --pane <PANE_ID> targets another (required with --machine)."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).into()).collect()
    }

    #[test]
    fn parses_ask_with_one_based_recommendation() {
        let command = parse_card_args(
            "ask",
            &args(&[
                "SHE-12",
                "Monthly or yearly?",
                "--option",
                "monthly",
                "--option",
                "yearly",
                "--recommend",
                "2",
                "--pane",
                "w1:p1",
            ]),
        )
        .unwrap();
        assert_eq!(
            command.method,
            Method::LedgerAsk(LedgerAskParams {
                pane_id: "w1:p1".into(),
                item_id: "SHE-12".into(),
                text: "Monthly or yearly?".into(),
                options: vec!["monthly".into(), "yearly".into()],
                recommended: Some(1),
            })
        );
        for bad in [&["0"][..], &["3"][..], &["x"][..]] {
            let mut input = args(&[
                "SHE-12", "q?", "--option", "a", "--option", "b", "--pane", "p",
            ]);
            input.push("--recommend".into());
            input.extend(args(bad));
            assert!(parse_card_args("ask", &input).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn parses_inferred_update_and_rejects_questions_without_it() {
        let command = parse_card_args(
            "update",
            &args(&[
                "SHE-1",
                "--inferred",
                "--status",
                "running",
                "--clear-questions",
                "--pane",
                "w1:p1",
            ]),
        )
        .unwrap();
        let Method::LedgerUpdate(params) = command.method else {
            panic!("expected ledger update");
        };
        assert!(params.inferred);
        assert_eq!(params.inferred_questions, Some(Vec::new()));
        assert!(parse_card_args(
            "update",
            &args(&["SHE-1", "--inferred-question", "q?", "--pane", "p"])
        )
        .is_err());
        assert!(
            parse_card_args("update", &args(&["SHE-1", "--option", "a", "--pane", "p"])).is_err()
        );
        assert!(parse_card_args("decide", &args(&["SHE-1", "--pane", "p"])).is_err());
    }

    #[test]
    fn list_json_flag_and_optional_pane() {
        let command = parse_card_args("list", &args(&["--json", "--pane", "w2:p3"])).unwrap();
        assert!(command.json);
        assert_eq!(
            command.method,
            Method::LedgerList(LedgerListParams {
                pane_id: Some("w2:p3".into())
            })
        );
    }

    #[test]
    fn older_servers_are_named_plainly() {
        assert!(no_ledger_on_server(&serde_json::json!({
            "code": "invalid_request",
            "message": "invalid request: unknown variant `ledger.update`, expected one of …"
        })));
        assert!(!no_ledger_on_server(&serde_json::json!({
            "code": "pane_not_found",
            "message": "pane w1:p9 not found"
        })));
    }

    #[test]
    fn list_text_shows_questions_options_and_decisions() {
        let text = format_list(
            &serde_json::json!({"panes": [{"pane_id": "w1:p1", "items": [
                {"id": "SHE-12", "title": "Stripe checkout", "status": "dev",
                 "questions": [{"id": 1, "text": "Plan?", "options": ["monthly", "yearly"],
                                "recommended": 1}],
                 "decisions": [{"text": "Use Checkout"}]}
            ]}]}),
        );
        assert_eq!(
            text,
            "w1:p1\n  SHE-12  Stripe checkout  [dev]  ?1\n    ? [1] Plan?  1) monthly  2) yearly ★\n    ✓ Use Checkout\n"
        );
    }

    #[test]
    fn list_text_never_prints_control_characters() {
        let text = format_list(
            &serde_json::json!({"panes": [{"pane_id": "w1:p1", "items": [
                {"id": "SHE-1", "title": "red \u{1b}]52;c;Zm9v\u{7}", "summary": "a\nb\u{9b}2J"}
            ]}]}),
        );
        assert_eq!(text, "w1:p1\n  SHE-1  red  ]52;c;Zm9v \n    a b 2J\n");
    }
}
