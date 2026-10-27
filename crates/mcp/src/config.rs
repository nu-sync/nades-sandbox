use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub sandbox_binary: PathBuf,
    pub packs_dir: PathBuf,
    pub runs_dir: PathBuf,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let sandbox_binary = std::env::var_os("NADES_SANDBOX_BINARY")
            .map(PathBuf::from)
            .unwrap_or(std::env::current_exe()?.with_file_name("nades-sandbox"));
        // the sandbox's own setting (docs/standalone-api.md §8): NADES_SANDBOX_PACKS, [assets] packs, the platform's cache
        let packs_dir = nades_config::packs(None)?.path;
        Ok(Self {
            sandbox_binary,
            packs_dir,
            runs_dir: PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                .join(".cache/nades/sandbox/mcp"),
        })
    }

    pub fn from_args(args: impl IntoIterator<Item = String>) -> anyhow::Result<Self> {
        let mut config = Self::from_env()?;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let destination = match arg.as_str() {
                "--sandbox-binary" => &mut config.sandbox_binary,
                "--packs-dir" => &mut config.packs_dir,
                "--runs-dir" => &mut config.runs_dir,
                _ => anyhow::bail!(
                    "unknown option {arg}; use --sandbox-binary PATH, --packs-dir PATH, or --runs-dir PATH"
                ),
            };
            *destination = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("{arg} needs a path"))?
                .into();
        }
        Ok(config)
    }
}
