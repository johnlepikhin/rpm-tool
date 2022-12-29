use std::fmt;

use anyhow::{anyhow, Context, Result};
use clap::{Args, Parser, Subcommand};
use slog::{o, Drain};
use slog_scope::error;

mod config;
pub mod digest;
pub mod lazy_result;
mod repodata;

const CONFIG_DEFAULT_PATH: &str = "/etc/rpm-tool.yaml";

#[derive(Clone, Debug, clap::ValueEnum)]
enum DumpFormat {
    Yaml,
    Json,
    RepodataXml,
}

impl DumpFormat {
    pub fn dump<T>(&self, v: &T) -> Result<String>
    where
        T: serde::Serialize,
    {
        let r = match self {
            DumpFormat::Yaml => serde_yaml::to_string(v)?,
            DumpFormat::Json => serde_json::to_string(v)?,
            DumpFormat::RepodataXml => quick_xml::se::to_string(v)?,
        };
        Ok(r)
    }
}

impl fmt::Display for DumpFormat {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// Dump metadata of RPM file
#[derive(Args)]
struct CmdRpmDump {
    #[arg(short, long, default_value_t = DumpFormat::Yaml, value_enum)]
    format: DumpFormat,
    file: std::path::PathBuf,
}

impl CmdRpmDump {
    fn run(&self) -> Result<()> {
        let mut rpm_file = std::fs::File::open(&self.file)?;
        let mut buf_reader = std::io::BufReader::new(&rpm_file);
        let pkg = rpm::RPMPackage::parse(&mut buf_reader)
            .map_err(|err| anyhow!("{}", err.to_string()))?;

        let file_sha = crate::digest::file_sha128(&mut rpm_file)?;
        let rpm = crate::repodata::primary::Package::of_rpm_package(
            &pkg,
            &self.file,
            &file_sha,
            &regex::Regex::new(".*").unwrap(),
        )?;
        let s = self.format.dump(&rpm)?;
        println!("{}", s);
        Ok(())
    }
}

/// Operations on single RPM file
#[derive(Subcommand)]
enum CmdRpm {
    Dump(CmdRpmDump),
}

impl CmdRpm {
    fn run(&self, _config: &crate::config::Config) -> Result<()> {
        match self {
            CmdRpm::Dump(v) => v.run(),
        }
    }
}

/// Generate RPM repository in given directory
#[derive(Args)]
struct CmdRepositoryGenerate {
    #[clap(long)]
    fileslists: bool,
    path: std::path::PathBuf,
}

impl From<&CmdRepositoryGenerate> for crate::repodata::RepodataOptions {
    fn from(v: &CmdRepositoryGenerate) -> Self {
        Self {
            generate_fileslists: v.fileslists,
            path: v.path.clone(),
        }
    }
}

impl CmdRepositoryGenerate {
    pub fn run(&self, config: &crate::config::Config) -> Result<()> {
        let repodata = crate::repodata::Repodata {
            config: &config.repodata,
            options: self.into(),
        };
        repodata.generate(&self.path)
    }
}

/// Operations on RPM repository
#[derive(Subcommand)]
enum CmdRepository {
    Generate(CmdRepositoryGenerate),
}

impl CmdRepository {
    fn run(&self, config: &crate::config::Config) -> Result<()> {
        match self {
            Self::Generate(v) => v.run(config),
        }
    }
}

#[derive(Subcommand)]
enum CommandLine {
    /// Dump parsed config file. Helps to find typos
    DumpConfig,
    /// Operations on single RPM file
    #[clap(subcommand)]
    Rpm(CmdRpm),
    #[clap(subcommand)]
    Repository(CmdRepository),
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Application {
    /// Path to configuration file
    #[clap(short, default_value = CONFIG_DEFAULT_PATH)]
    config_path: String,
    /// Subcommand
    #[clap(subcommand)]
    command: CommandLine,
}

impl Application {
    fn init_syslog_logger(log_level: slog::Level) -> Result<slog_scope::GlobalLoggerGuard> {
        let logger = slog_syslog::SyslogBuilder::new()
            .facility(slog_syslog::Facility::LOG_USER)
            .level(log_level)
            .unix("/dev/log")
            .start()?;

        let logger = slog::Logger::root(logger.fuse(), o!());
        Ok(slog_scope::set_global_logger(logger))
    }

    fn init_env_logger() -> Result<slog_scope::GlobalLoggerGuard> {
        Ok(slog_envlogger::init()?)
    }

    fn init_logger(&self, config: &config::Config) -> Result<slog_scope::GlobalLoggerGuard> {
        if std::env::var("RUST_LOG").is_ok() {
            Self::init_env_logger()
        } else {
            Self::init_syslog_logger(config.log_level.into())
        }
    }

    fn run_command(&self, config: config::Config) -> Result<()> {
        match &self.command {
            CommandLine::DumpConfig => {
                let config =
                    serde_yaml::to_string(&config).with_context(|| "Failed to dump config")?;
                println!("{}", config);
                Ok(())
            }
            CommandLine::Rpm(v) => v.run(&config),
            CommandLine::Repository(v) => v.run(&config),
        }
    }

    pub fn run(&self) {
        let config = config::Config::read(&self.config_path).expect("Config");
        let _logger_guard = self.init_logger(&config).expect("Logger");

        if let Err(err) = self.run_command(config) {
            error!("Failed with error: {:#}", err);
        }
    }
}

fn main() {
    Application::parse().run();
}
