use std::{
    env, fs,
    io::{self, Write},
    os::unix::fs::PermissionsExt,
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant},
};

use crate::{
    adapters::storage::SqliteStore,
    config::{
        AppConfig, AssistantPaths, IdentityConfig, LlmConfig, TelegramConfig,
        write_identity_config, write_llm_config, write_telegram_config,
    },
    core::{
        identity::{UserProfile, write_assistant_profile},
        inbound_queue::dispatch_due_once,
        memory::{search_memories, summarize_session},
        rag,
        scheduler::{add_job, list_jobs, run_due_jobs},
        session::{fetch_session, list_sessions},
        service::run_chat_session,
        skills::{create_skill, install_skill_file, list_skills, run_skill},
        tasks::{add_task, complete_task, list_tasks, update_task},
        telegram::{
            adapter_from_config, approve_pairing_code, deny_pairing_code, list_pending_pairings,
            poll_for_first_pairing, process_telegram_once, runtime_status, send_message,
            session_key,
        },
        tools::{ToolExecutor, add_tool, remove_tool},
        voice::{DEFAULT_VOICE_SESSION, doctor_voice, run_voice_turn},
    },
};

pub fn run_from_env() -> Result<(), String> {
    let args = env::args().collect::<Vec<_>>();
    let paths = AssistantPaths::discover()?;
    let config = AppConfig::load(&paths)?;
    if args.len() <= 1 && !onboarding_complete(&config) {
        run_telegram_onboarding_wizard(&paths)?;
        return Ok(());
    }

    let output = run(args, paths)?;
    if !output.is_empty() {
        println!("{output}");
    }
    Ok(())
}

pub fn run(args: Vec<String>, paths: AssistantPaths) -> Result<String, String> {
    let config = AppConfig::load(&paths)?;
    let store = SqliteStore::new(&paths)?;
    let tools = ToolExecutor::new(config.tools.allowlist.clone(), paths.root.clone());

    if args.len() <= 1 {
        if onboarding_complete(&config) {
            return render_help_text(&paths, &config, &store);
        }
        return Ok(onboarding_needed_text());
    }

    match args[1].as_str() {
        "chat" => run_chat(&args[2..], &paths, &config, &store),
        "search" => run_search(&args[2..], &store, config.memory.memory_search_limit),
        "ingest" => run_ingest(&args[2..], &store),
        "task" => run_task(&args[2..], &store),
        "tool" => run_tool(&args[2..], &paths, &config, &tools),
        "skill" => run_skill_command(&args[2..], &paths, &tools),
        "memory" => run_memory(
            &args[2..],
            &store,
            config.memory.memory_search_limit,
            config.memory.memory_ttl_days,
        ),
        "summarize" => {
            let session = args.get(2).map(String::as_str).unwrap_or("default");
            summarize_session(&paths, &store, session)
        }
        "schedule" => run_schedule(&args[2..], &store),
        "jobs" => run_jobs(&args[2..], &paths, &store, &config, &tools),
        "rag" => run_rag(&args[2..], &store),
        "serve" => run_serve(&args[2..], &paths, &store, &config, &tools),
        "telegram" => run_telegram_command(&args[2..], &paths, &store, &config),
        "voice" => run_voice_command(&args[2..], &paths, &store, &config),
        "session" => run_session_command(&args[2..], &store),
        "queue" => run_queue_command(&args[2..], &store),
        "doctor" => run_doctor(&paths, &config, &store),
        "onboard" => run_onboard(&args[2..], &paths),
        "help" | "--help" | "-h" => render_help_text(&paths, &config, &store),
        other => Err(format!(
            "unknown command `{other}`\n\n{}",
            render_help_text(&paths, &config, &store)?
        )),
    }
}

fn run_chat(
    args: &[String],
    paths: &AssistantPaths,
    config: &AppConfig,
    store: &SqliteStore,
) -> Result<String, String> {
    let session = value_after_flag(args, "--session").unwrap_or_else(|| "default".to_string());
    let stream = args.iter().any(|arg| arg == "--stream") || config.llm.stream;
    let message = if let Some(value) = value_after_flag(args, "--message") {
        value
    } else if !args.is_empty() {
        args.join(" ")
    } else {
        return Err("chat requires --message <text> or positional text".to_string());
    };

    let outcome = run_chat_session(paths, config, store, &session, &message, stream)?;
    let _compaction = outcome.compaction;
    Ok(outcome.response)
}

fn run_search(args: &[String], store: &SqliteStore, limit: usize) -> Result<String, String> {
    if args.is_empty() {
        return Err("search requires a query".to_string());
    }
    let query = args.join(" ");
    let memory_results = search_memories(store, &query, limit)?;
    let rag_results = rag::search(store, &query, limit)?;

    let mut lines = vec![format!("Search query: {query}")];
    lines.push("Memory hits:".to_string());
    if memory_results.is_empty() {
        lines.push("- none".to_string());
    } else {
        lines.extend(
            memory_results
                .into_iter()
                .map(|item| format!("- {} :: {}", item.title, item.body)),
        );
    }
    lines.push("RAG hits:".to_string());
    if rag_results.is_empty() {
        lines.push("- none".to_string());
    } else {
        lines.extend(rag_results.into_iter().map(|item| format!("- {item}")));
    }
    Ok(lines.join("\n"))
}

fn run_ingest(args: &[String], store: &SqliteStore) -> Result<String, String> {
    let target = args
        .first()
        .ok_or_else(|| "ingest requires a path".to_string())?;
    let count = rag::index_path(store, Path::new(target))?;
    Ok(format!("indexed {count} documents from {target}"))
}

fn run_task(args: &[String], store: &SqliteStore) -> Result<String, String> {
    match args.first().map(String::as_str) {
        Some("add") => {
            if args.len() < 2 {
                return Err("task add requires a title".to_string());
            }
            let priority = value_after_flag(args, "--priority")
                .map(|value| {
                    value
                        .parse::<i64>()
                        .map_err(|_| "priority must be numeric".to_string())
                })
                .transpose()?
                .unwrap_or(1);
            let title_parts = filter_flag_pair(args, &["--priority"]);
            if title_parts.is_empty() {
                return Err("task add requires a title".to_string());
            }
            add_task(store, &title_parts.join(" "), priority)
        }
        Some("list") => {
            let tasks = list_tasks(store)?;
            if tasks.is_empty() {
                return Ok("no tasks".to_string());
            }
            Ok(tasks
                .into_iter()
                .map(|task| {
                    let suffix = if task.notes.is_empty() {
                        String::new()
                    } else {
                        format!(" :: {}", task.notes)
                    };
                    format!(
                        "#{} [p{} {}] {}{}",
                        task.id, task.priority, task.status, task.title, suffix
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"))
        }
        Some("complete") => {
            let id = args
                .get(1)
                .ok_or_else(|| "task complete requires an id".to_string())?
                .parse()
                .map_err(|_| "task id must be numeric".to_string())?;
            complete_task(store, id)
        }
        Some("update") => {
            let id = args
                .get(1)
                .ok_or_else(|| "task update requires an id".to_string())?
                .parse::<i64>()
                .map_err(|_| "task id must be numeric".to_string())?;
            let title = value_after_flag(args, "--title");
            let status = value_after_flag(args, "--status");
            let priority = value_after_flag(args, "--priority")
                .map(|value| {
                    value
                        .parse::<i64>()
                        .map_err(|_| "priority must be numeric".to_string())
                })
                .transpose()?;
            let notes = value_after_flag(args, "--notes");
            update_task(
                store,
                id,
                title.as_deref(),
                status.as_deref(),
                priority,
                notes.as_deref(),
            )
        }
        _ => Err("task supports: add | list | complete | update".to_string()),
    }
}

fn run_tool(
    args: &[String],
    paths: &AssistantPaths,
    config: &AppConfig,
    tools: &ToolExecutor,
) -> Result<String, String> {
    match args.first().map(String::as_str) {
        Some("list") => {
            if tools.allowlist().is_empty() {
                return Ok("no tools allowlisted".to_string());
            }
            Ok(tools.allowlist().join("\n"))
        }
        Some("add") => {
            let command = args
                .get(1)
                .ok_or_else(|| "tool add requires a command".to_string())?;
            add_tool(paths, &config.tools, command)
        }
        Some("remove") => {
            let command = args
                .get(1)
                .ok_or_else(|| "tool remove requires a command".to_string())?;
            remove_tool(paths, &config.tools, command)
        }
        Some("run") => {
            let command = args
                .get(1)
                .ok_or_else(|| "tool run requires a command".to_string())?;
            tools.run(command, &args[2..].to_vec())
        }
        Some("read") => {
            let path = args
                .get(1)
                .ok_or_else(|| "tool read requires a relative path".to_string())?;
            tools.read_file(path)
        }
        Some("write") => {
            let path = args
                .get(1)
                .ok_or_else(|| "tool write requires a relative path".to_string())?;
            let contents = args
                .get(2)
                .ok_or_else(|| "tool write requires contents".to_string())?;
            tools.write_markdown(path, contents)
        }
        Some("append") => {
            let path = args
                .get(1)
                .ok_or_else(|| "tool append requires a relative path".to_string())?;
            let contents = args
                .get(2)
                .ok_or_else(|| "tool append requires contents".to_string())?;
            tools.append_markdown(path, contents)
        }
        _ => Err("tool supports: list | add | remove | run | read | write | append".to_string()),
    }
}

fn run_skill_command(
    args: &[String],
    paths: &AssistantPaths,
    tools: &ToolExecutor,
) -> Result<String, String> {
    match args.first().map(String::as_str) {
        Some("create") => {
            let name = args
                .get(1)
                .ok_or_else(|| "skill create requires a name".to_string())?;
            let description = value_after_flag(args, "--description").unwrap_or_default();
            let triggers = value_after_flag(args, "--triggers")
                .map(|value| split_cli_csv(&value))
                .unwrap_or_default();
            let required_tools = value_after_flag(args, "--tools")
                .map(|value| split_cli_csv(&value))
                .unwrap_or_default();
            let instructions = value_after_flag(args, "--instructions").unwrap_or_default();
            let steps = values_after_repeated_flag(args, "--step");
            create_skill(
                paths,
                name,
                &description,
                &triggers,
                &required_tools,
                &instructions,
                &steps,
            )
        }
        Some("install") => {
            let path = args
                .get(1)
                .ok_or_else(|| "skill install requires a markdown file path".to_string())?;
            install_skill_file(paths, Path::new(path))
        }
        Some("list") => {
            let skills = list_skills(paths)?;
            if skills.is_empty() {
                return Ok("no skills installed".to_string());
            }
            Ok(skills
                .into_iter()
                .map(|skill| {
                    format!(
                        "{} :: {} :: triggers=[{}] tools=[{}]",
                        skill.name,
                        skill.description,
                        skill.triggers.join(", "),
                        skill.tools.join(", ")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"))
        }
        Some("show") => {
            let name = args
                .get(1)
                .ok_or_else(|| "skill show requires a skill name".to_string())?;
            let skill = crate::core::skills::find_skill(paths, name)?;
            Ok(format!(
                "{}\n{}\ntriggers: {}\ntools: {}\n\n{}",
                skill.name,
                skill.description,
                skill.triggers.join(", "),
                skill.tools.join(", "),
                skill.instructions
            ))
        }
        Some("run") => {
            let selector = args
                .get(1)
                .ok_or_else(|| "skill run requires a skill name or `auto`".to_string())?;
            let task = args[2..].join(" ");
            if task.is_empty() {
                return Err("skill run requires a task".to_string());
            }
            run_skill(paths, tools, selector, &task)
        }
        _ => Err("skill supports: create | install | list | show | run".to_string()),
    }
}

fn run_memory(
    args: &[String],
    store: &SqliteStore,
    limit: usize,
    default_ttl_days: usize,
) -> Result<String, String> {
    match args.first().map(String::as_str) {
        Some("search") => {
            let query = args[1..].join(" ");
            if query.is_empty() {
                return Err("memory search requires a query".to_string());
            }
            let results = search_memories(store, &query, limit)?;
            if results.is_empty() {
                return Ok("no memory matches".to_string());
            }
            Ok(results
                .into_iter()
                .map(|item| format!("{} :: {}", item.title, item.body))
                .collect::<Vec<_>>()
                .join("\n"))
        }
        Some("add") => {
            if args.len() < 5 {
                return Err(
                    "memory add requires <kind> <title> <tags> <body> [--ttl-days N]".to_string(),
                );
            }
            let ttl_days = value_after_flag(args, "--ttl-days")
                .map(|value| {
                    value
                        .parse::<i64>()
                        .map_err(|_| "ttl-days must be numeric".to_string())
                })
                .transpose()?
                .unwrap_or(default_ttl_days as i64);
            let kind = args[1].clone();
            let title = args[2].clone();
            let tags = args[3].clone();
            let body_parts = filter_flag_pair(args, &["--ttl-days"]);
            if body_parts.len() < 3 {
                return Err("memory add requires <kind> <title> <tags> <body>".to_string());
            }
            let body = body_parts[2..].join(" ");
            crate::core::memory::add_memory_with_expiry(
                store,
                &kind,
                "cli",
                &title,
                &body,
                &tags,
                1.0,
                Some(ttl_days),
            )?;
            Ok(format!("memory added: {title}"))
        }
        _ => Err("memory supports: search | add".to_string()),
    }
}

fn run_schedule(args: &[String], store: &SqliteStore) -> Result<String, String> {
    match args.first().map(String::as_str) {
        Some("add") => {
            if args.len() < 4 {
                return Err("schedule add requires <name> <every_minutes> <action...>".to_string());
            }
            let minutes = args[2]
                .parse()
                .map_err(|_| "schedule interval must be numeric minutes".to_string())?;
            add_job(store, &args[1], minutes, &args[3..].join(" "))
        }
        Some("list") => {
            let jobs = list_jobs(store)?;
            if jobs.is_empty() {
                return Ok("no scheduled jobs".to_string());
            }
            Ok(jobs
                .into_iter()
                .map(|job| {
                    format!(
                        "#{} [{}] every {}m :: {}",
                        job.id,
                        if job.enabled { "enabled" } else { "disabled" },
                        job.every_minutes,
                        job.action
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"))
        }
        _ => Err("schedule supports: add | list".to_string()),
    }
}

fn run_jobs(
    args: &[String],
    paths: &AssistantPaths,
    store: &SqliteStore,
    config: &AppConfig,
    tools: &ToolExecutor,
) -> Result<String, String> {
    if args.len() == 2 && args[0] == "run" && args[1] == "--once" {
        let logs = run_due_jobs(paths, store, &config.scheduler, tools)?;
        if logs.is_empty() {
            return Ok("no jobs were due".to_string());
        }
        return Ok(logs.join("\n"));
    }
    Err("jobs supports: run --once".to_string())
}

fn run_rag(args: &[String], store: &SqliteStore) -> Result<String, String> {
    match args.first().map(String::as_str) {
        Some("index") => run_ingest(&args[1..], store),
        Some("search") => {
            let query = args[1..].join(" ");
            if query.is_empty() {
                return Err("rag search requires a query".to_string());
            }
            let results = rag::search(store, &query, 8)?;
            Ok(if results.is_empty() {
                "no rag matches".to_string()
            } else {
                results.join("\n")
            })
        }
        _ => Err("rag supports: index | search".to_string()),
    }
}

fn run_serve(
    args: &[String],
    paths: &AssistantPaths,
    store: &SqliteStore,
    config: &AppConfig,
    tools: &ToolExecutor,
) -> Result<String, String> {
    if args.iter().any(|arg| arg == "--once") {
        return Ok(run_service_tick(paths, store, config, tools, Some(0))?.join("\n"));
    }
    if let Some(iterations) = value_after_flag(args, "--iterations") {
        let iterations = iterations
            .parse::<usize>()
            .map_err(|_| "--iterations must be numeric".to_string())?;
        let mut lines = Vec::new();
        for _ in 0..iterations {
            lines.extend(run_service_tick(
                paths,
                store,
                config,
                tools,
                Some(config.telegram.poll_timeout_secs.min(1)),
            )?);
        }
        return Ok(lines.join("\n"));
    }

    let scheduler_interval = service_scheduler_interval(config);
    let mut next_scheduler_tick = Instant::now();
    loop {
        let mut logs = Vec::new();
        let now = Instant::now();
        if now >= next_scheduler_tick {
            logs.extend(run_due_jobs(paths, store, &config.scheduler, tools)?);
            next_scheduler_tick = Instant::now() + scheduler_interval;
        }

        logs.extend(dispatch_due_once(paths, store, config)?);
        logs.extend(process_telegram_once(
            paths,
            store,
            config,
            service_telegram_timeout(config, Instant::now(), next_scheduler_tick),
        )?);
        logs.extend(dispatch_due_once(paths, store, config)?);
        if !logs.is_empty() {
            println!("{}", logs.join("\n"));
        }

        if !config.telegram.enabled {
            let sleep_for = next_scheduler_tick.saturating_duration_since(Instant::now());
            if !sleep_for.is_zero() {
                thread::sleep(sleep_for);
            }
        }
    }
}

fn run_telegram_command(
    args: &[String],
    paths: &AssistantPaths,
    store: &SqliteStore,
    config: &AppConfig,
) -> Result<String, String> {
    match args.first().map(String::as_str) {
        Some("status") => {
            let status = runtime_status(store, &config.telegram)?;
            Ok(format!(
                "Telegram enabled: {}\nOwner user id: {}\nAllowed users: {}\nPending pairings: {}\nLast update id: {}",
                status.enabled,
                status
                    .owner_user_id
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "none".to_string()),
                if status.allowed_user_ids.is_empty() {
                    "none".to_string()
                } else {
                    status
                        .allowed_user_ids
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                },
                status.pending_count,
                status.last_update_id
            ))
        }
        Some("pending") => {
            let pending = list_pending_pairings(store)?;
            if pending.is_empty() {
                return Ok("no pending Telegram approvals".to_string());
            }
            Ok(pending
                .into_iter()
                .map(|item| {
                    format!(
                        "{} :: {} :: expires {}",
                        item.code,
                        item.username
                            .strip_prefix('@')
                            .map(|_| item.username.clone())
                            .unwrap_or_else(|| {
                                if item.username.is_empty() {
                                    item.first_name.clone()
                                } else {
                                    format!("@{}", item.username)
                                }
                            }),
                        item.expires_at
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"))
        }
        Some("approve") => {
            let code = args
                .get(1)
                .ok_or_else(|| "telegram approve requires a pairing code".to_string())?;
            let approved = approve_pairing_code(paths, store, &config.telegram, code)?;
            let _ = send_message(
                paths,
                &AppConfig::load(paths)?.telegram,
                approved.chat_id,
                "Telegram access approved. You can chat with the assistant now.",
            );
            Ok(format!(
                "approved {} ({})",
                if approved.username.is_empty() {
                    approved.first_name
                } else {
                    format!("@{}", approved.username)
                },
                approved.code
            ))
        }
        Some("deny") => {
            let code = args
                .get(1)
                .ok_or_else(|| "telegram deny requires a pairing code".to_string())?;
            deny_pairing_code(store, code)
        }
        _ => Err("telegram supports: status | pending | approve <code> | deny <code>".to_string()),
    }
}

fn run_voice_command(
    args: &[String],
    paths: &AssistantPaths,
    store: &SqliteStore,
    config: &AppConfig,
) -> Result<String, String> {
    match args.first().map(String::as_str) {
        Some("run") => {
            if !args.iter().any(|arg| arg == "--once") {
                return Err("voice run supports: --once".to_string());
            }
            let session =
                value_after_flag(args, "--session").unwrap_or_else(|| DEFAULT_VOICE_SESSION.into());
            let output = run_voice_turn(paths, config, store, &session)?;
            Ok(render_voice_turn_output(&output))
        }
        Some("serve") => run_voice_serve(&args[1..], paths, store, config),
        Some("doctor") => Ok(render_voice_doctor(paths, config)),
        _ => Err("voice supports: run --once | serve | doctor".to_string()),
    }
}

fn run_voice_serve(
    args: &[String],
    paths: &AssistantPaths,
    store: &SqliteStore,
    config: &AppConfig,
) -> Result<String, String> {
    let session =
        value_after_flag(args, "--session").unwrap_or_else(|| DEFAULT_VOICE_SESSION.into());
    let iterations = value_after_flag(args, "--iterations")
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| "--iterations must be numeric".to_string())
        })
        .transpose()?;
    let mut completed = 0usize;
    loop {
        wait_for_voice_trigger(config)?;
        let output = run_voice_turn(paths, config, store, &session)?;
        println!("{}", render_voice_turn_output(&output));
        completed += 1;
        if iterations.is_some_and(|limit| completed >= limit) {
            return Ok(format!("voice serve completed {completed} turn(s)"));
        }
    }
}

fn wait_for_voice_trigger(config: &AppConfig) -> Result<(), String> {
    let command = config.voice.push_to_talk_command.trim();
    if !command.is_empty() {
        let mut parts = command.split_whitespace();
        let executable = parts
            .next()
            .ok_or_else(|| "push_to_talk_command is empty".to_string())?;
        let output = Command::new(executable)
            .args(parts)
            .output()
            .map_err(|error| format!("failed to run push_to_talk_command: {error}"))?;
        if output.status.success() {
            return Ok(());
        }
        return Err(format!(
            "push_to_talk_command failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    println!("Press Enter to capture a voice turn.");
    let mut buffer = String::new();
    io::stdin()
        .read_line(&mut buffer)
        .map_err(|error| format!("failed to read push-to-talk input: {error}"))?;
    Ok(())
}

fn render_voice_turn_output(output: &crate::core::voice::VoiceTurnOutput) -> String {
    let mut lines = vec![format!("voice session: {}", output.session)];
    if output.skipped {
        lines.push("voice turn skipped: empty transcript".to_string());
    }
    if let Some(transcript) = &output.transcript {
        lines.push(format!("transcript: {transcript}"));
    }
    if let Some(response) = &output.response {
        lines.push(format!("assistant: {response}"));
    }
    if let Some(path) = &output.input_audio_path {
        lines.push(format!("input audio: {}", path.display()));
    }
    if let Some(path) = &output.output_audio_path {
        lines.push(format!("reply audio: {}", path.display()));
    }
    for error in &output.errors {
        lines.push(format!("voice warning: {error}"));
    }
    lines.join("\n")
}

fn render_voice_doctor(paths: &AssistantPaths, config: &AppConfig) -> String {
    let mut lines = vec![
        "Voice doctor report".to_string(),
        format!("voice enabled: {}", config.voice.enabled),
        format!("trigger mode: {}", config.voice.trigger_mode),
    ];
    lines.extend(
        doctor_voice(paths, &config.voice)
            .into_iter()
            .map(|(name, ok)| check_line(&name, ok)),
    );
    lines.push(format!(
        "input device: {}",
        empty_as_default(&config.voice.input_device)
    ));
    lines.push(format!(
        "output device: {}",
        empty_as_default(&config.voice.output_device)
    ));
    lines.join("\n")
}

fn run_session_command(args: &[String], store: &SqliteStore) -> Result<String, String> {
    match args.first().map(String::as_str) {
        Some("list") | None => {
            let sessions = list_sessions(store)?;
            if sessions.is_empty() {
                return Ok("no sessions recorded".to_string());
            }
            Ok(sessions
                .into_iter()
                .map(|item| {
                    format!(
                        "{} :: surface={} kind={} state={} reply={} updated={}",
                        item.session.session_id,
                        item.session.surface,
                        item.session.session_kind.as_str(),
                        item.session.state.as_str(),
                        item.session.reply_policy.as_str(),
                        item.updated_at
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"))
        }
        Some("show") => {
            let session_id = args
                .get(1)
                .ok_or_else(|| "session show requires a session id".to_string())?;
            let item = fetch_session(store, session_id)?
                .ok_or_else(|| format!("unknown session `{session_id}`"))?;
            Ok(format!(
                concat!(
                    "session_id: {}\n",
                    "surface: {}\n",
                    "peer_id: {}\n",
                    "chat_id: {}\n",
                    "kind: {}\n",
                    "activation: {}\n",
                    "reply_policy: {}\n",
                    "state: {}\n",
                    "tool_policy: trusted={} memory_write={} commands={}\n",
                    "model_policy: task={} fallback={} degraded={}\n",
                    "last_message_at: {}\n",
                    "updated_at: {}"
                ),
                item.session.session_id,
                item.session.surface,
                empty_as_default(item.session.peer_id.as_deref().unwrap_or_default()),
                empty_as_default(item.session.chat_id.as_deref().unwrap_or_default()),
                item.session.session_kind.as_str(),
                item.session.activation_mode.as_str(),
                item.session.reply_policy.as_str(),
                item.session.state.as_str(),
                item.session.tool_policy.trusted,
                item.session.tool_policy.allow_memory_write,
                if item.session.tool_policy.allowlisted_commands.is_empty() {
                    "none".to_string()
                } else {
                    item.session.tool_policy.allowlisted_commands.join(", ")
                },
                item.session.model_policy.task,
                if item.session.model_policy.fallback_order.is_empty() {
                    "none".to_string()
                } else {
                    item.session.model_policy.fallback_order.join(", ")
                },
                item.session.model_policy.allow_degraded_fallback,
                item.last_message_at
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "none".to_string()),
                item.updated_at
            ))
        }
        _ => Err("session supports: list | show <session-id>".to_string()),
    }
}

fn run_queue_command(args: &[String], store: &SqliteStore) -> Result<String, String> {
    match args.first().map(String::as_str) {
        Some("status") | None => {
            let queued = queue_status_count(store, "queued")?;
            let running = queue_status_count(store, "running")?;
            let done = queue_status_count(store, "done")?;
            let failed = queue_status_count(store, "failed")?;
            let dropped = queue_status_count(store, "dropped")?;
            let leases = store
                .scalar("SELECT COUNT(*) FROM inbound_queue_leases;")?
                .unwrap_or_else(|| "0".to_string());
            Ok(format!(
                "queue queued={} running={} done={} failed={} dropped={} active_leases={}",
                queued, running, done, failed, dropped, leases
            ))
        }
        Some("show") => {
            let rows = store.query(
                "SELECT id, surface, session_id, status, created_at, available_at,
                        COALESCE(started_at, ''), COALESCE(finished_at, ''),
                        merged_count, substr(replace(message_text, char(10), ' '), 1, 120)
                 FROM inbound_queue
                 ORDER BY created_at DESC
                 LIMIT 20;",
            )?;
            if rows.is_empty() {
                return Ok("queue is empty".to_string());
            }
            Ok(rows
                .into_iter()
                .filter(|row| row.len() >= 10)
                .map(|row| {
                    format!(
                        "#{} [{} {}] {} :: created={} available={} started={} finished={} merged={} :: {}",
                        row[0], row[1], row[3], row[2], row[4], row[5], empty_as_default(&row[6]),
                        empty_as_default(&row[7]), row[8], row[9]
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"))
        }
        _ => Err("queue supports: status | show".to_string()),
    }
}

fn queue_status_count(store: &SqliteStore, status: &str) -> Result<String, String> {
    store
        .scalar(&format!(
            "SELECT COUNT(*) FROM inbound_queue WHERE status = '{}';",
            status
        ))
        .map(|value| value.unwrap_or_else(|| "0".to_string()))
}

fn run_doctor(paths: &AssistantPaths, config: &AppConfig, store: &SqliteStore) -> Result<String, String> {
    let mut lines = vec!["Doctor report".to_string()];
    lines.push(check_line("config/", writable_dir(&paths.config_dir)));
    lines.push(check_line("data/", writable_dir(&paths.data_dir)));
    lines.push(check_line("sqlite3", command_available("sqlite3")));
    lines.push(check_line("curl", command_available("curl")));
    lines.push(check_line(
        "llama-cli path",
        executable_path(&config.llm.binary_path),
    ));
    lines.push(check_line(
        "GGUF model path",
        Path::new(&config.llm.model_path).exists(),
    ));
    lines.push(format!(
        "effective llm context/predict: {}/{}",
        if config.llm.context_size == 0 {
            2048
        } else {
            config.llm.context_size.min(2048)
        },
        if config.llm.predict_tokens == 0 {
            192
        } else {
            config.llm.predict_tokens.min(192)
        }
    ));
    lines.push(format!(
        "queue enabled/mode: {}/{}",
        config.messages.queue.enabled, config.messages.queue.mode
    ));
    lines.push(format!(
        "queue caps: global={} per-session={}",
        config.messages.queue.global_max_concurrency,
        config.messages.queue.per_session_cap
    ));
    lines.push(format!(
        "telegram reply chunk chars: {}",
        config.messages.reply.telegram_chunk_chars
    ));
    lines.push(format!(
        "session count: {}",
        list_sessions(store)?.len()
    ));
    lines.push(format!("queue health: {}", run_queue_command(&["status".into()], store)?));

    lines.push(String::new());
    lines.push(render_voice_doctor(paths, config));

    let token_state = match config.telegram.resolve_bot_token(paths)? {
        Some(token) => {
            let probe = TelegramConfig {
                enabled: true,
                bot_token: token,
                bot_token_file: String::new(),
                ..config.telegram.clone()
            };
            match adapter_from_config(paths, &probe)?.unwrap().get_me() {
                Ok(bot) => format!(
                    "ok ({})",
                    if bot.username.is_empty() {
                        bot.first_name
                    } else {
                        format!("@{}", bot.username)
                    }
                ),
                Err(error) => format!("failed ({error})"),
            }
        }
        None => "missing".to_string(),
    };
    lines.push(format!("bot token: {token_state}"));
    Ok(lines.join("\n"))
}

fn run_onboard(args: &[String], paths: &AssistantPaths) -> Result<String, String> {
    match args.first().map(String::as_str) {
        None | Some("identity") => {
            run_identity_onboarding_wizard(paths)?;
            Ok("identity onboarding complete".to_string())
        }
        Some("telegram") => {
            run_telegram_onboarding_wizard(paths)?;
            Ok("telegram onboarding complete".to_string())
        }
        _ => Err("onboard supports: identity | telegram".to_string()),
    }
}

fn run_identity_onboarding_wizard(paths: &AssistantPaths) -> Result<(), String> {
    let current = AppConfig::load(paths)?;

    println!("Identity onboarding");
    println!();
    println!("Set the assistant identity and the user profile stored locally on this Pi.");
    println!("Press Enter to keep a default value or leave an optional field blank.");
    println!();

    let identity = IdentityConfig {
        name: prompt_line_with_default("Assistant name", &current.identity.name)?,
        style: prompt_line_with_default("Assistant reply style", &current.identity.style)?,
        system_instruction: current.identity.system_instruction.clone(),
    };
    let user = UserProfile {
        name: prompt_line_with_default("Your name", "User")?,
        telegram_handle: prompt_line("Telegram / handle (optional): ")?,
        role: prompt_line("Role / what you do (optional): ")?,
        about: prompt_line("What should the assistant know about you? (optional): ")?,
        goals: prompt_line("Current goals or active projects (optional): ")?,
        preferences: prompt_line_with_default(
            "Preferred reply style",
            "direct, concise, practical",
        )?,
    };

    write_identity_onboarding(paths, &identity, &user)?;
    println!("Updated config/identity.json and data/profiles/assistant.md.");
    Ok(())
}

fn write_identity_onboarding(
    paths: &AssistantPaths,
    identity: &IdentityConfig,
    user: &UserProfile,
) -> Result<(), String> {
    write_identity_config(paths, identity)?;
    write_assistant_profile(paths, identity, user)
}

fn run_telegram_onboarding_wizard(paths: &AssistantPaths) -> Result<(), String> {
    let store = SqliteStore::new(paths)?;
    let current = AppConfig::load(paths)?;

    println!("Telegram onboarding");
    println!();
    println!("1. Open Telegram and talk to @BotFather.");
    println!("2. Create a bot with /newbot.");
    println!("3. Copy the bot token or store it in a local text file.");
    println!("4. Keep this terminal open while you DM the bot.");
    println!();

    let (telegram_config, bot_label) = loop {
        let entry = prompt_line("Bot token or token file path: ")?;
        let candidate = build_telegram_candidate(&current.telegram, entry.trim());
        let Some(adapter) = adapter_from_config(paths, &candidate)? else {
            println!("That value did not resolve to a token. Try again.");
            continue;
        };
        match adapter.get_me() {
            Ok(bot) => {
                let label = if bot.username.is_empty() {
                    bot.first_name.clone()
                } else {
                    format!("@{}", bot.username)
                };
                println!("Validated Telegram bot: {label} (id {})", bot.id);
                break (candidate, label);
            }
            Err(error) => {
                println!("Token validation failed: {error}");
            }
        }
    };

    let llm_config = resolve_local_llm_config(paths, &current.llm);
    println!(
        "Local llama.cpp defaults\n- llama-cli: {}\n- model: {}",
        llm_config.binary_path, llm_config.model_path
    );
    if !Path::new(&llm_config.binary_path).exists() || !Path::new(&llm_config.model_path).exists() {
        return Err("local llama.cpp defaults were not found; fix the binary/model paths and rerun onboarding".to_string());
    }

    println!("Writing config/llm.json and config/telegram.json");
    write_llm_config(paths, &llm_config)?;
    write_telegram_config(paths, &telegram_config)?;

    println!("Starting Telegram long polling for {bot_label}.");
    println!("Send the bot a direct message now. Waiting for the first private text message.");
    let wait_config = TelegramConfig {
        poll_timeout_secs: 5,
        ..telegram_config.clone()
    };
    let pending = poll_for_first_pairing(paths, &store, &wait_config, 24)?
        .ok_or_else(|| "timed out waiting for the first Telegram DM".to_string())?;

    println!(
        "Pending Telegram sender\n- code: {}\n- user: {}\n- user_id: {}",
        pending.code,
        if pending.username.is_empty() {
            pending.first_name.clone()
        } else {
            format!("@{}", pending.username)
        },
        pending.user_id
    );
    if !prompt_yes_no("Approve this Telegram user? [Y/n]: ", true)? {
        return Err("onboarding stopped before approval".to_string());
    }

    let approved = approve_pairing_code(paths, &store, &telegram_config, &pending.code)?;
    println!();
    println!("Tell the assistant about you so it can answer personal context questions correctly.");
    println!("Press Enter to keep a default value or skip an optional field.");
    let user_profile = collect_onboarding_user_profile(&approved)?;
    let current = AppConfig::load(paths)?;
    write_assistant_profile(paths, &current.identity, &user_profile)?;
    println!("Updated the assistant profile with your user details.");

    let onboarded = AppConfig::load(paths)?;
    let outcome = run_chat_session(
        paths,
        &onboarded,
        &store,
        &session_key(approved.user_id),
        "Send a short welcome message confirming Telegram pairing and local-first operation.",
        false,
    )?;
    send_message(
        paths,
        &onboarded.telegram,
        approved.chat_id,
        &outcome.response,
    )?;
    println!("Sent a live test reply back to Telegram.");

    if prompt_yes_no("Show systemd install commands now? [Y/n]: ", true)? {
        println!();
        println!("sudo cp deploy/ai_assistant.service /etc/systemd/system/ai_assistant.service");
        println!("sudo systemctl daemon-reload");
        println!("sudo systemctl enable --now ai_assistant.service");
        println!();
    }

    println!("Onboarding complete. Use `assistant serve` to keep Telegram polling active.");
    Ok(())
}

fn render_help_text(
    paths: &AssistantPaths,
    config: &AppConfig,
    store: &SqliteStore,
) -> Result<String, String> {
    let onboarding = if onboarding_complete(config) {
        "complete"
    } else {
        "pending"
    };
    let telegram = runtime_status(store, &config.telegram)?;
    let llama_ready =
        executable_path(&config.llm.binary_path) && Path::new(&config.llm.model_path).exists();
    Ok([
        format!("Onboarding: {onboarding}"),
        format!(
            "Telegram: {}",
            if config.telegram.enabled {
                format!(
                    "enabled, owner={}, allowed={}, pending={}",
                    telegram
                        .owner_user_id
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "none".to_string()),
                    telegram.allowed_user_ids.len(),
                    telegram.pending_count
                )
            } else {
                "disabled".to_string()
            }
        ),
        format!(
            "Local llama.cpp: {}",
            if llama_ready {
                format!("ready ({})", config.llm.binary_path)
            } else {
                format!("missing ({})", config.llm.binary_path)
            }
        ),
        String::new(),
        "Next steps:".to_string(),
        if onboarding_complete(config) {
            "assistant serve".to_string()
        } else {
            "assistant onboard".to_string()
        },
        "assistant onboard telegram".to_string(),
        "assistant doctor".to_string(),
        "assistant telegram status".to_string(),
        "assistant session list".to_string(),
        "assistant queue status".to_string(),
        "assistant voice doctor".to_string(),
        "assistant voice run --once".to_string(),
        String::new(),
        "Commands:".to_string(),
        "assistant chat --message <text> [--session <id>] [--stream]".to_string(),
        "assistant onboard".to_string(),
        "assistant onboard identity".to_string(),
        "assistant onboard telegram".to_string(),
        "assistant telegram status".to_string(),
        "assistant telegram pending".to_string(),
        "assistant telegram approve <code>".to_string(),
        "assistant telegram deny <code>".to_string(),
        "assistant session list".to_string(),
        "assistant session show <session-id>".to_string(),
        "assistant queue status".to_string(),
        "assistant queue show".to_string(),
        "assistant voice run --once [--session <id>]".to_string(),
        "assistant voice serve [--session <id>] [--iterations N]".to_string(),
        "assistant voice doctor".to_string(),
        "assistant doctor".to_string(),
        "assistant search <query>".to_string(),
        "assistant ingest <path>".to_string(),
        "assistant task add <title> [--priority N]".to_string(),
        "assistant task list".to_string(),
        "assistant task complete <id>".to_string(),
        "assistant task update <id> [--title text] [--status text] [--priority N] [--notes text]"
            .to_string(),
        "assistant tool list".to_string(),
        "assistant tool add <command>".to_string(),
        "assistant tool remove <command>".to_string(),
        "assistant tool run <command> [args...]".to_string(),
        "assistant tool read <relative-path>".to_string(),
        "assistant tool write <relative-path.md> <contents>".to_string(),
        "assistant tool append <relative-path.md> <contents>".to_string(),
        "assistant skill create <name> [--description text] [--triggers csv] [--tools csv] [--instructions text] [--step kind: payload]".to_string(),
        "assistant skill install <path.md>".to_string(),
        "assistant skill list".to_string(),
        "assistant skill show <name>".to_string(),
        "assistant skill run <name|auto> <task>".to_string(),
        "assistant memory search <query>".to_string(),
        "assistant memory add <kind> <title> <tags> <body> [--ttl-days N]".to_string(),
        "assistant summarize [session]".to_string(),
        "assistant schedule add <name> <every_minutes> <action>".to_string(),
        "assistant schedule list".to_string(),
        "assistant jobs run --once".to_string(),
        "assistant rag index <path>".to_string(),
        "assistant rag search <query>".to_string(),
        "assistant serve [--once | --iterations N]".to_string(),
        format!("Root: {}", paths.root.display()),
    ]
    .join("\n"))
}

fn onboarding_needed_text() -> String {
    [
        "Onboarding: pending",
        "Run `assistant onboard` for local identity onboarding or `assistant onboard telegram` for the Telegram setup wizard.",
    ]
    .join("\n")
}

fn onboarding_complete(config: &AppConfig) -> bool {
    config.telegram.onboarding_complete()
        && Path::new(&config.llm.binary_path).exists()
        && Path::new(&config.llm.model_path).exists()
}

fn resolve_local_llm_config(paths: &AssistantPaths, current: &LlmConfig) -> LlmConfig {
    let mut config = current.clone();
    if config.binary_path.trim().is_empty() || !Path::new(&config.binary_path).exists() {
        config.binary_path = LlmConfig::local_first(paths).binary_path;
    }
    if config.model_path.trim().is_empty() || !Path::new(&config.model_path).exists() {
        config.model_path = LlmConfig::local_first(paths).model_path;
    }
    config.prefer_http = false;
    if config.endpoint.trim().is_empty() {
        config.endpoint = "http://127.0.0.1:8080/v1/chat/completions".to_string();
    }
    if config.health_endpoint.trim().is_empty() {
        config.health_endpoint = "http://127.0.0.1:8080/health".to_string();
    }
    config.context_size = if config.context_size == 0 {
        2048
    } else {
        config.context_size.min(2048)
    };
    config.predict_tokens = if config.predict_tokens == 0 {
        192
    } else {
        config.predict_tokens.min(192)
    };
    config.stream = false;
    config
}

fn build_telegram_candidate(current: &TelegramConfig, input: &str) -> TelegramConfig {
    let looks_like_path = input.contains('/') || input.contains('.') || input.ends_with(".txt");
    TelegramConfig {
        enabled: true,
        bot_token: if looks_like_path {
            String::new()
        } else {
            input.to_string()
        },
        bot_token_file: if looks_like_path {
            input.to_string()
        } else {
            String::new()
        },
        poll_timeout_secs: 30,
        owner_user_id: None,
        allowed_user_ids: Vec::new(),
        pairing_enabled: true,
        pairing_code_ttl_minutes: 15,
        api_base_url: current.api_base_url.clone(),
    }
}

fn run_service_tick(
    paths: &AssistantPaths,
    store: &SqliteStore,
    config: &AppConfig,
    tools: &ToolExecutor,
    telegram_timeout: Option<usize>,
) -> Result<Vec<String>, String> {
    let mut logs = run_due_jobs(paths, store, &config.scheduler, tools)?;
    logs.extend(dispatch_due_once(paths, store, config)?);
    logs.extend(process_telegram_once(
        paths,
        store,
        config,
        telegram_timeout,
    )?);
    logs.extend(dispatch_due_once(paths, store, config)?);
    Ok(logs)
}

fn service_scheduler_interval(config: &AppConfig) -> Duration {
    Duration::from_secs(config.scheduler.poll_seconds.max(1) as u64)
}

fn service_telegram_timeout(
    config: &AppConfig,
    now: Instant,
    next_scheduler_tick: Instant,
) -> Option<usize> {
    if !config.telegram.enabled {
        return None;
    }
    let remaining = next_scheduler_tick.saturating_duration_since(now);
    let remaining_secs = remaining.as_secs().max(1) as usize;
    Some(config.telegram.poll_timeout_secs.min(remaining_secs))
}

fn writable_dir(path: &Path) -> bool {
    if fs::create_dir_all(path).is_err() {
        return false;
    }
    let probe = path.join(".assistant-write-check");
    let result = fs::write(&probe, "ok").is_ok();
    let _ = fs::remove_file(probe);
    result
}

fn command_available(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn executable_path(path: &str) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn check_line(name: &str, ok: bool) -> String {
    format!("{name}: {}", if ok { "ok" } else { "missing" })
}

fn empty_as_default(value: &str) -> &str {
    if value.trim().is_empty() {
        "default"
    } else {
        value
    }
}

fn prompt_line(prompt: &str) -> Result<String, String> {
    print!("{prompt}");
    io::stdout()
        .flush()
        .map_err(|error| format!("failed to flush stdout: {error}"))?;
    let mut buffer = String::new();
    io::stdin()
        .read_line(&mut buffer)
        .map_err(|error| format!("failed to read stdin: {error}"))?;
    Ok(buffer.trim().to_string())
}

fn prompt_line_with_default(prompt: &str, default: &str) -> Result<String, String> {
    let value = prompt_line(&format!("{prompt} [{default}]: "))?;
    if value.trim().is_empty() {
        Ok(default.trim().to_string())
    } else {
        Ok(value)
    }
}

fn prompt_yes_no(prompt: &str, default_yes: bool) -> Result<bool, String> {
    let answer = prompt_line(prompt)?;
    if answer.is_empty() {
        return Ok(default_yes);
    }
    match answer.to_ascii_lowercase().as_str() {
        "y" | "yes" => Ok(true),
        "n" | "no" => Ok(false),
        _ => Ok(default_yes),
    }
}

fn collect_onboarding_user_profile(
    approved: &crate::core::telegram::PendingPairing,
) -> Result<UserProfile, String> {
    let default_name = if !approved.first_name.trim().is_empty() {
        approved.first_name.trim().to_string()
    } else if !approved.username.trim().is_empty() {
        approved.username.trim().to_string()
    } else {
        "User".to_string()
    };
    let default_preferences = "direct, concise, practical";
    let name = prompt_line_with_default("Your name", &default_name)?;
    let role = prompt_line("Role / what you do (optional): ")?;
    let about = prompt_line("What should the assistant know about you? (optional): ")?;
    let goals = prompt_line("Current goals or active projects (optional): ")?;
    let preferences = prompt_line_with_default("Preferred reply style", default_preferences)?;

    Ok(UserProfile {
        name,
        telegram_handle: if approved.username.trim().is_empty() {
            String::new()
        } else {
            format!("@{}", approved.username.trim().trim_start_matches('@'))
        },
        role,
        about,
        goals,
        preferences,
    })
}

fn value_after_flag(args: &[String], flag: &str) -> Option<String> {
    args.windows(2)
        .find(|window| window[0] == flag)
        .map(|window| window[1].clone())
}

fn values_after_repeated_flag(args: &[String], flag: &str) -> Vec<String> {
    args.windows(2)
        .filter(|window| window[0] == flag)
        .map(|window| window[1].clone())
        .collect()
}

fn split_cli_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn filter_flag_pair(args: &[String], flags: &[&str]) -> Vec<String> {
    let mut filtered = Vec::new();
    let mut skip_next = false;
    for item in args.iter().skip(1) {
        if skip_next {
            skip_next = false;
            continue;
        }
        if flags.iter().any(|flag| item == flag) {
            skip_next = true;
            continue;
        }
        filtered.push(item.clone());
    }
    filtered
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use std::{
        fs,
        io::{Read, Write},
        net::TcpListener,
        os::unix::fs::PermissionsExt,
        sync::{Arc, Mutex},
        thread,
    };

    use crate::{
        cli::run,
        config::{AssistantPaths, IdentityConfig},
        core::identity::{UserProfile, render_assistant_profile},
        util::unique_temp_dir,
    };

    use super::{
        onboarding_complete, render_help_text, service_scheduler_interval,
        service_telegram_timeout, write_identity_onboarding,
    };

    #[test]
    fn cli_supports_task_schedule_and_rag_commands() {
        let root = unique_temp_dir("assistant-cli-test");
        let paths = AssistantPaths::new(root.clone());
        paths.ensure_defaults().unwrap();
        fs::write(
            paths.notes_dir.join("sample.md"),
            "# Sample\n\nLocal memory note.",
        )
        .unwrap();

        let task_output = run(
            vec![
                "assistant".into(),
                "task".into(),
                "add".into(),
                "Check sensors".into(),
            ],
            paths.clone(),
        )
        .unwrap();
        assert!(task_output.contains("task added"));

        let rag_output = run(
            vec![
                "assistant".into(),
                "rag".into(),
                "index".into(),
                paths.notes_dir.to_string_lossy().to_string(),
            ],
            paths.clone(),
        )
        .unwrap();
        assert!(rag_output.contains("indexed"));

        let schedule_output = run(
            vec![
                "assistant".into(),
                "schedule".into(),
                "add".into(),
                "daily".into(),
                "5".into(),
                "task".into(),
                "add".into(),
                "Review notes".into(),
            ],
            paths.clone(),
        )
        .unwrap();
        assert!(schedule_output.contains("scheduled job"));
    }

    #[test]
    fn cli_supports_task_updates_and_tool_markdown_operations() {
        let root = unique_temp_dir("assistant-cli-tools");
        let paths = AssistantPaths::new(root);
        paths.ensure_defaults().unwrap();

        let added = run(
            vec![
                "assistant".into(),
                "task".into(),
                "add".into(),
                "Review logs".into(),
                "--priority".into(),
                "2".into(),
            ],
            paths.clone(),
        )
        .unwrap();
        assert!(added.contains("task added"));

        let updated = run(
            vec![
                "assistant".into(),
                "task".into(),
                "update".into(),
                "1".into(),
                "--status".into(),
                "in_progress".into(),
                "--notes".into(),
                "check CPU temperature first".into(),
            ],
            paths.clone(),
        )
        .unwrap();
        assert!(updated.contains("task updated"));

        run(
            vec![
                "assistant".into(),
                "tool".into(),
                "write".into(),
                "data/notes/runtime.md".into(),
                "# Runtime".into(),
            ],
            paths.clone(),
        )
        .unwrap();
        run(
            vec![
                "assistant".into(),
                "tool".into(),
                "append".into(),
                "data/notes/runtime.md".into(),
                "\n\nstill offline".into(),
            ],
            paths.clone(),
        )
        .unwrap();
        let content = run(
            vec![
                "assistant".into(),
                "tool".into(),
                "read".into(),
                "data/notes/runtime.md".into(),
            ],
            paths,
        )
        .unwrap();
        assert!(content.contains("still offline"));
    }

    #[test]
    fn cli_supports_explicit_memory_add_and_search() {
        let root = unique_temp_dir("assistant-cli-memory");
        let paths = AssistantPaths::new(root);
        paths.ensure_defaults().unwrap();

        let added = run(
            vec![
                "assistant".into(),
                "memory".into(),
                "add".into(),
                "preference".into(),
                "coffee".into(),
                "user,preference".into(),
                "prefers concise morning summaries".into(),
            ],
            paths.clone(),
        )
        .unwrap();
        assert!(added.contains("memory added"));

        let searched = run(
            vec![
                "assistant".into(),
                "memory".into(),
                "search".into(),
                "concise".into(),
            ],
            paths,
        )
        .unwrap();
        assert!(searched.contains("coffee"));
    }

    #[test]
    fn cli_supports_adding_tools_and_running_skills() {
        let root = unique_temp_dir("assistant-cli-skills");
        let paths = AssistantPaths::new(root);
        paths.ensure_defaults().unwrap();

        let tool_added = run(
            vec![
                "assistant".into(),
                "tool".into(),
                "add".into(),
                "printf".into(),
            ],
            paths.clone(),
        )
        .unwrap();
        assert!(tool_added.contains("tool allowlisted"));

        let tool_list = run(
            vec!["assistant".into(), "tool".into(), "list".into()],
            paths.clone(),
        )
        .unwrap();
        assert!(tool_list.contains("printf"));

        let skill_added = run(
            vec![
                "assistant".into(),
                "skill".into(),
                "create".into(),
                "Runtime Note".into(),
                "--description".into(),
                "Capture runtime notes".into(),
                "--triggers".into(),
                "note,runtime".into(),
                "--tools".into(),
                "printf".into(),
                "--instructions".into(),
                "Append the requested task to a markdown note.".into(),
                "--step".into(),
                "append_markdown: data/notes/runtime-skill.md | {{task}}".into(),
                "--step".into(),
                "command: printf ok".into(),
            ],
            paths.clone(),
        )
        .unwrap();
        assert!(skill_added.contains("runtime-note"));

        let output = run(
            vec![
                "assistant".into(),
                "skill".into(),
                "run".into(),
                "auto".into(),
                "write".into(),
                "a".into(),
                "runtime".into(),
                "note".into(),
            ],
            paths.clone(),
        )
        .unwrap();
        assert!(output.contains("skill: runtime-note"));
        assert!(output.contains("command `printf` => ok"));

        let note = run(
            vec![
                "assistant".into(),
                "tool".into(),
                "read".into(),
                "data/notes/runtime-skill.md".into(),
            ],
            paths,
        )
        .unwrap();
        assert!(note.contains("write a runtime note"));
    }

    #[test]
    fn no_args_reports_onboarding_pending_for_fresh_workspace() {
        let root = unique_temp_dir("assistant-cli-onboarding");
        let paths = AssistantPaths::new(root);
        paths.ensure_defaults().unwrap();

        let output = run(vec!["assistant".into()], paths).unwrap();
        assert!(output.contains("Onboarding: pending"));
    }

    #[test]
    fn rendered_assistant_profile_includes_user_details() {
        let updated = render_assistant_profile(
            &IdentityConfig {
                name: "Kumo".into(),
                style: "direct, concise, practical".into(),
                system_instruction: "Stay local".into(),
            },
            &UserProfile {
                name: "David Bong".into(),
                telegram_handle: "@davidb2021".into(),
                role: "HardCoder".into(),
                about: "Builds local AI systems".into(),
                goals: "Fix the Telegram assistant".into(),
                preferences: "direct, concise, practical".into(),
            },
        );

        assert!(updated.contains("# Assistant Profile"));
        assert!(updated.contains("## User Profile"));
        assert!(updated.contains("Name: David Bong"));
        assert!(updated.contains("Telegram: @davidb2021"));
        assert!(updated.contains("Role: HardCoder"));
        assert!(updated.contains("Fix the Telegram assistant"));
    }

    #[test]
    fn write_identity_onboarding_updates_identity_and_profile_files() {
        let root = unique_temp_dir("assistant-cli-identity-onboard");
        let paths = AssistantPaths::new(root);
        paths.ensure_defaults().unwrap();

        write_identity_onboarding(
            &paths,
            &IdentityConfig {
                name: "Ayaka".into(),
                style: "direct, concise, practical".into(),
                system_instruction: "Stay local".into(),
            },
            &UserProfile {
                name: "HardCoder".into(),
                telegram_handle: "@davidb2021".into(),
                role: "Builder".into(),
                about: "Builds local AI systems".into(),
                goals: "Fix the assistant".into(),
                preferences: "direct, concise, practical".into(),
            },
        )
        .unwrap();

        let identity = fs::read_to_string(paths.config_dir.join("identity.json")).unwrap();
        assert!(identity.contains("\"name\": \"Ayaka\""));
        assert!(identity.contains("direct, concise, practical"));

        let profile = fs::read_to_string(paths.profiles_dir.join("assistant.md")).unwrap();
        assert!(profile.contains("Name: Ayaka"));
        assert!(profile.contains("## User Profile"));
        assert!(profile.contains("Name: HardCoder"));
    }

    #[test]
    fn help_renders_status_for_onboarded_workspace() {
        let root = unique_temp_dir("assistant-cli-help");
        let paths = AssistantPaths::new(root.clone());
        paths.ensure_defaults().unwrap();
        fs::create_dir_all(root.join("config")).unwrap();
        let binary = root.join("llama-cli");
        let model = root.join("model.gguf");
        fs::write(&binary, "#!/bin/sh\nprintf 'ok\\n'\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(&model, "mock").unwrap();
        fs::write(
            paths.config_dir.join("telegram.json"),
            r#"{
  "enabled": true,
  "bot_token": "123:token",
  "bot_token_file": "",
  "poll_timeout_secs": 1,
  "owner_user_id": "42",
  "allowed_user_ids": ["42"],
  "pairing_enabled": true,
  "pairing_code_ttl_minutes": 15,
  "api_base_url": "https://api.telegram.org"
}"#,
        )
        .unwrap();
        fs::write(
            paths.config_dir.join("llm.json"),
            format!(
                r#"{{
  "prefer_http": false,
  "endpoint": "http://127.0.0.1:8080/v1/chat/completions",
  "health_endpoint": "http://127.0.0.1:8080/health",
  "model": "mock",
  "binary_path": "{}",
  "model_path": "{}",
  "threads": 1,
  "context_size": 64,
  "predict_tokens": 16,
  "timeout_secs": 1,
  "retries": 0,
  "stream": false
}}"#,
                binary.display(),
                model.display()
            ),
        )
        .unwrap();
        let config = crate::config::AppConfig::load(&paths).unwrap();
        let store = crate::adapters::storage::SqliteStore::new(&paths).unwrap();

        assert!(onboarding_complete(&config));
        let help = render_help_text(&paths, &config, &store).unwrap();
        assert!(help.contains("Onboarding: complete"));
        assert!(help.contains("assistant onboard"));
        assert!(help.contains("assistant telegram status"));
    }

    #[test]
    fn service_timeout_caps_telegram_poll_to_scheduler_deadline() {
        let root = unique_temp_dir("assistant-cli-timeout-cap");
        let paths = AssistantPaths::new(root);
        paths.ensure_defaults().unwrap();
        let mut config = crate::config::AppConfig::load(&paths).unwrap();
        config.telegram.enabled = true;
        let now = Instant::now();
        let timeout = service_telegram_timeout(&config, now, now + Duration::from_secs(5)).unwrap();
        assert_eq!(timeout, 5);
    }

    #[test]
    fn service_timeout_is_disabled_without_telegram() {
        let root = unique_temp_dir("assistant-cli-timeout-disabled");
        let paths = AssistantPaths::new(root);
        paths.ensure_defaults().unwrap();
        let mut config = crate::config::AppConfig::load(&paths).unwrap();
        config.telegram.enabled = false;
        let now = Instant::now();
        assert_eq!(
            service_telegram_timeout(&config, now, now + Duration::from_secs(5)),
            None
        );
        assert_eq!(service_scheduler_interval(&config), Duration::from_secs(30));
    }

    #[test]
    fn serve_once_processes_scheduler_and_telegram_work() {
        let root = unique_temp_dir("assistant-cli-serve-once");
        let paths = AssistantPaths::new(root.clone());
        paths.ensure_defaults().unwrap();
        let store = crate::adapters::storage::SqliteStore::new(&paths).unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen_requests = requests.clone();
        let server = thread::spawn(move || {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0u8; 4096];
                let size = stream.read(&mut buffer).unwrap();
                let request = String::from_utf8_lossy(&buffer[..size]);
                let method = if request.contains("/getUpdates") {
                    "getUpdates"
                } else if request.contains("/sendChatAction") {
                    "sendChatAction"
                } else if request.contains("/sendMessage") {
                    "sendMessage"
                } else {
                    "other"
                };
                seen_requests.lock().unwrap().push(method.to_string());
                let body = if request.contains("/getUpdates") {
                    r#"{"ok":true,"result":[{"update_id":7,"message":{"message_id":1,"from":{"id":42,"first_name":"David","username":"dbong"},"chat":{"id":42,"type":"private"},"date":1710000000,"text":"hello"}}]}"#
                } else if request.contains("/sendChatAction") {
                    r#"{"ok":true,"result":true}"#
                } else if request.contains("/sendMessage") {
                    r#"{"ok":true,"result":{"message_id":2}}"#
                } else {
                    r#"{"ok":true,"result":{"id":999,"username":"testbot","first_name":"Test"}} "#
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body.trim()
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });

        let binary = root.join("llama-cli");
        let model = root.join("model.gguf");
        fs::write(&binary, "#!/bin/sh\nprintf 'telegram hello\\n'\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(&model, "mock").unwrap();

        fs::write(
            paths.config_dir.join("telegram.json"),
            format!(
                r#"{{
  "enabled": true,
  "bot_token": "123:token",
  "bot_token_file": "",
  "poll_timeout_secs": 0,
  "owner_user_id": "42",
  "allowed_user_ids": ["42"],
  "pairing_enabled": true,
  "pairing_code_ttl_minutes": 15,
  "api_base_url": "http://{}"
}}"#,
                address
            ),
        )
        .unwrap();
        fs::write(
            paths.config_dir.join("llm.json"),
            format!(
                r#"{{
  "prefer_http": false,
  "endpoint": "http://127.0.0.1:9/v1/chat/completions",
  "health_endpoint": "http://127.0.0.1:9/health",
  "model": "mock",
  "binary_path": "{}",
  "model_path": "{}",
  "threads": 1,
  "context_size": 64,
  "predict_tokens": 16,
  "timeout_secs": 1,
  "retries": 0,
  "stream": false
}}"#,
                binary.display(),
                model.display()
            ),
        )
        .unwrap();

        crate::core::scheduler::add_job(&store, "job-one", 0, "task add first").unwrap();

        let output = run(
            vec!["assistant".into(), "serve".into(), "--once".into()],
            paths.clone(),
        )
        .unwrap();
        assert!(output.contains("job-one"));
        assert!(output.contains("queued Telegram message"));
        assert!(output.contains("delivered queued telegram reply"));
        assert_eq!(
            crate::core::memory::recent_turns(&store, "telegram:dm:42", 4)
                .unwrap()
                .len(),
            2
        );

        server.join().unwrap();
        let requests = requests.lock().unwrap().clone();
        assert_eq!(
            requests,
            vec!["getUpdates", "sendChatAction", "sendMessage"]
        );
    }

    #[test]
    fn telegram_reply_does_not_leak_compaction_notice() {
        let root = unique_temp_dir("assistant-cli-telegram-compaction");
        let paths = AssistantPaths::new(root.clone());
        paths.ensure_defaults().unwrap();

        fs::write(
            paths.config_dir.join("memory.json"),
            r#"{
  "recent_turn_limit": 4,
  "compact_after_turns": 2,
  "retain_recent_turns": 1,
  "token_budget": 128,
  "memory_search_limit": 4,
  "memory_ttl_days": 30
}"#,
        )
        .unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen_requests = requests.clone();
        let server = thread::spawn(move || {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0u8; 4096];
                let size = stream.read(&mut buffer).unwrap();
                let request = String::from_utf8_lossy(&buffer[..size]).to_string();
                seen_requests.lock().unwrap().push(request.clone());
                let body = if request.contains("/getUpdates") {
                    r#"{"ok":true,"result":[{"update_id":8,"message":{"message_id":1,"from":{"id":42,"first_name":"David","username":"dbong"},"chat":{"id":42,"type":"private"},"date":1710000000,"text":"Can you search internet?"}}]}"#
                } else if request.contains("/sendChatAction") {
                    r#"{"ok":true,"result":true}"#
                } else {
                    r#"{"ok":true,"result":{"message_id":2}}"#
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });

        fs::write(
            paths.config_dir.join("telegram.json"),
            format!(
                r#"{{
  "enabled": true,
  "bot_token": "123:token",
  "bot_token_file": "",
  "poll_timeout_secs": 0,
  "owner_user_id": "42",
  "allowed_user_ids": ["42"],
  "pairing_enabled": true,
  "pairing_code_ttl_minutes": 15,
  "api_base_url": "http://{}"
}}"#,
                address
            ),
        )
        .unwrap();

        let output = run(
            vec!["assistant".into(), "serve".into(), "--once".into()],
            paths.clone(),
        )
        .unwrap();
        assert!(output.contains("queued Telegram message"));
        assert!(output.contains("delivered queued telegram reply"));

        server.join().unwrap();
        let requests = requests.lock().unwrap().clone();
        let send_message = requests
            .iter()
            .find(|request| request.contains("/sendMessage"))
            .unwrap();
        assert!(send_message.contains("No internet search tool is configured"));
        assert!(!send_message.contains("compacted session"));
    }
}
