use crate::api::schema::RuntimeAddParams;
use crate::runtime::RuntimeKind;

pub(super) fn run_runtime_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(|arg| arg.as_str()) else {
        print_runtime_help();
        return Ok(2);
    };

    match subcommand {
        "list" => runtime_list(&args[1..]),
        "get" => runtime_get(&args[1..]),
        "add" => runtime_add(&args[1..]),
        "remove" | "rm" => runtime_remove(&args[1..]),
        "help" | "--help" | "-h" => {
            print_runtime_help();
            Ok(0)
        }
        _ => {
            print_runtime_help();
            Ok(2)
        }
    }
}

fn runtime_list(args: &[String]) -> std::io::Result<i32> {
    if !args.is_empty() {
        eprintln!("usage: herdr runtime list");
        return Ok(2);
    }

    super::runtime::runtime_list()
}

fn runtime_get(args: &[String]) -> std::io::Result<i32> {
    let Some(runtime_id) = args.first() else {
        eprintln!("usage: herdr runtime get <runtime_id>");
        return Ok(2);
    };
    if args.len() != 1 {
        eprintln!("usage: herdr runtime get <runtime_id>");
        return Ok(2);
    }

    super::runtime::runtime_get(runtime_id.clone())
}

fn runtime_remove(args: &[String]) -> std::io::Result<i32> {
    let Some(runtime_id) = args.first() else {
        eprintln!("usage: herdr runtime remove <runtime_id>");
        return Ok(2);
    };
    if args.len() != 1 {
        eprintln!("usage: herdr runtime remove <runtime_id>");
        return Ok(2);
    }

    super::runtime::runtime_remove(runtime_id.clone())
}

fn runtime_add(args: &[String]) -> std::io::Result<i32> {
    let Some(id) = args.first().cloned() else {
        eprintln!(
            "usage: herdr runtime add <id> (--ssh HOST | --socket PATH) [--session NAME] [--label TEXT]"
        );
        return Ok(2);
    };

    let mut kind = None;
    let mut target = None;
    let mut session = None;
    let mut label = None;

    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--ssh" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --ssh");
                    return Ok(2);
                };
                if kind.is_some() {
                    eprintln!("specify only one of --ssh or --socket");
                    return Ok(2);
                }
                kind = Some(RuntimeKind::RemoteSsh);
                target = Some(value.clone());
                index += 2;
            }
            "--socket" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --socket");
                    return Ok(2);
                };
                if kind.is_some() {
                    eprintln!("specify only one of --ssh or --socket");
                    return Ok(2);
                }
                kind = Some(RuntimeKind::Socket);
                target = Some(value.clone());
                index += 2;
            }
            "--session" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --session");
                    return Ok(2);
                };
                session = Some(value.clone());
                index += 2;
            }
            "--label" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --label");
                    return Ok(2);
                };
                label = Some(value.clone());
                index += 2;
            }
            other => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
        }
    }

    let Some(kind) = kind else {
        eprintln!("usage: herdr runtime add <id> (--ssh HOST | --socket PATH) [--session NAME] [--label TEXT]");
        return Ok(2);
    };

    if kind == RuntimeKind::Socket && session.is_some() {
        eprintln!("--session is only valid with --ssh");
        return Ok(2);
    }

    super::runtime::runtime_add(RuntimeAddParams {
        id,
        kind,
        label,
        target,
        session,
    })
}

fn print_runtime_help() {
    eprintln!("herdr runtime commands:");
    eprintln!("  herdr runtime list");
    eprintln!("  herdr runtime get <runtime_id>");
    eprintln!(
        "  herdr runtime add <id> (--ssh HOST | --socket PATH) [--session NAME] [--label TEXT]"
    );
    eprintln!("  herdr runtime remove <runtime_id>");
}
