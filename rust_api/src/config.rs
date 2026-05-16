use std::{
    env,
    path::{Path, PathBuf},
};

const MODEL_FILENAME: &str = "SmolLM2-135M-Instruct.Q4_K_M.gguf";
const DEFAULT_MODEL_ALIAS: &str = "smollm2-135m-instruct";

#[derive(Clone, Debug)]
pub struct Config {
    pub binary: PathBuf,
    pub model: PathBuf,
    pub model_alias: String,
    pub host: String,
    pub port: u16,
    pub threads: usize,
    pub timeout_secs: u64,
    pub context_size: usize,
    pub max_concurrency: usize,
}

impl Config {
    pub fn from_env() -> Self {
        let project_roots = runtime_roots();
        let binary = first_existing_path("LLAMA_BINARY", &binary_candidates(&project_roots), true);
        let model = first_existing_path("LLAMA_MODEL", &model_candidates(&project_roots), false);

        Self {
            binary,
            model,
            model_alias: env::var("LLAMA_MODEL_ALIAS")
                .unwrap_or_else(|_| DEFAULT_MODEL_ALIAS.to_string()),
            host: env::var("LLAMA_HOST")
                .or_else(|_| env::var("FLASK_HOST"))
                .unwrap_or_else(|_| "0.0.0.0".to_string()),
            port: parse_u16(
                env::var("LLAMA_PORT")
                    .or_else(|_| env::var("FLASK_PORT"))
                    .ok(),
                8080,
                1,
            ),
            threads: parse_usize(env::var("LLAMA_THREADS").ok(), default_threads(), 1),
            timeout_secs: parse_u64(env::var("LLAMA_TIMEOUT").ok(), 180, 1),
            context_size: parse_usize(env::var("LLAMA_CONTEXT_SIZE").ok(), 128, 32),
            max_concurrency: parse_usize(env::var("LLAMA_MAX_CONCURRENCY").ok(), 1, 1),
        }
    }
}

fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map(|value| value.get().min(4))
        .unwrap_or(1)
        .max(1)
}

fn parse_usize(value: Option<String>, default: usize, minimum: usize) -> usize {
    value
        .and_then(|raw| raw.parse::<usize>().ok())
        .unwrap_or(default)
        .max(minimum)
}

fn parse_u64(value: Option<String>, default: u64, minimum: u64) -> u64 {
    value
        .and_then(|raw| raw.parse::<u64>().ok())
        .unwrap_or(default)
        .max(minimum)
}

fn parse_u16(value: Option<String>, default: u16, minimum: u16) -> u16 {
    value
        .and_then(|raw| raw.parse::<u16>().ok())
        .unwrap_or(default)
        .max(minimum)
}

fn first_existing_path(env_name: &str, candidates: &[PathBuf], executable: bool) -> PathBuf {
    if let Ok(explicit) = env::var(env_name) {
        return expand_home(explicit);
    }

    candidates
        .iter()
        .find(|candidate| candidate.exists() && (!executable || is_executable(candidate)))
        .cloned()
        .unwrap_or_else(|| candidates.first().cloned().unwrap_or_default())
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }

    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn expand_home(raw: String) -> PathBuf {
    if raw == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from(raw));
    }

    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }

    PathBuf::from(raw)
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

fn runtime_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if let Ok(current_dir) = env::current_dir() {
        roots.push(current_dir);
    }

    if let Ok(current_exe) = env::current_exe() {
        if let Some(parent) = current_exe.parent() {
            roots.push(parent.to_path_buf());
            if let Some(grandparent) = parent.parent() {
                roots.push(grandparent.to_path_buf());
            }
        }
    }

    roots.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf(),
    );

    dedupe_paths(roots)
}

fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut unique = Vec::new();

    for path in paths {
        if !unique.iter().any(|existing| existing == &path) {
            unique.push(path);
        }
    }

    unique
}

fn binary_candidates(project_roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    for project_root in project_roots {
        candidates.extend([
            project_root.join("ec2_bin/llama-cli"),
            project_root.join("ec2-build-rpi-bin/llama-cli"),
            project_root.join("llama.cpp-bin/llama-cli"),
            project_root.join("llama.cpp/build/bin/llama-cli"),
            project_root.join("bin/llama-cli"),
            project_root.join("llama-cli"),
        ]);
    }

    if let Some(home) = home_dir() {
        candidates.extend([
            home.join("projects/llama.cpp/build/bin/llama-cli"),
            home.join("ec2_bin/llama-cli"),
            home.join("ec2-build-rpi-bin/llama-cli"),
            home.join("llama.cpp-bin/llama-cli"),
            home.join("llama.cpp/build/bin/llama-cli"),
        ]);
    }

    candidates
}

fn model_candidates(project_roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    for project_root in project_roots {
        candidates.extend([
            project_root.join(format!("ec2_bin/{MODEL_FILENAME}")),
            project_root.join(format!("ec2-build-rpi-bin/{MODEL_FILENAME}")),
            project_root.join(format!("llama.cpp-bin/{MODEL_FILENAME}")),
            project_root.join(MODEL_FILENAME),
            project_root.join(format!("models/{MODEL_FILENAME}")),
            project_root.join(format!("llama-run/models/{MODEL_FILENAME}")),
        ]);
    }

    if let Some(home) = home_dir() {
        candidates.extend([
            home.join(format!("models/{MODEL_FILENAME}")),
            home.join(format!("projects/llama.cpp/models/{MODEL_FILENAME}")),
            home.join(format!("projects/llama.cpp/{MODEL_FILENAME}")),
            home.join(format!("ec2_bin/{MODEL_FILENAME}")),
            home.join(format!("ec2-build-rpi-bin/{MODEL_FILENAME}")),
            home.join(format!("llama.cpp-bin/{MODEL_FILENAME}")),
            home.join(format!("llama-run/models/{MODEL_FILENAME}")),
        ]);
    }

    candidates
}
