use std::path::PathBuf;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum, error::ErrorKind};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Parser, PartialEq, Eq)]
#[command(name = "hoshino", version, about = "A bounded terminal project card")]
pub struct Cli {
    #[arg(long, global = true)]
    pub full: bool,
    #[arg(long, global = true)]
    pub json: bool,
    #[arg(long, value_name = "PATH", global = true)]
    pub config: Option<PathBuf>,
    #[arg(long, value_enum, global = true)]
    pub color: Option<ColorMode>,
    #[arg(long, value_name = "PATH", global = true, conflicts_with = "no_image")]
    pub image: Option<PathBuf>,
    #[arg(long, global = true, conflicts_with = "image")]
    pub no_image: bool,
    #[arg(long, value_enum, global = true)]
    pub protocol: Option<Protocol>,
    #[arg(long, hide = true, global = true)]
    pub hook_context: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Clone, Debug, Subcommand, PartialEq, Eq)]
pub enum Command {
    Live,
    Hook { shell: Shell },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Protocol {
    Auto,
    Kitty,
    Iterm2,
    Sixel,
    Halfblocks,
    None,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
    Nu,
    Powershell,
}

impl Cli {
    pub fn try_parse_from<I, T>(args: I) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        let cli = <Self as Parser>::try_parse_from(args)?;
        cli.validate()?;
        Ok(cli)
    }

    pub fn parse_and_validate() -> Self {
        let cli = <Self as Parser>::parse();
        if let Err(error) = cli.validate() {
            error.exit();
        }
        cli
    }

    fn validate(&self) -> Result<(), clap::Error> {
        if matches!(self.command, Some(Command::Live)) && self.json {
            return Err(Self::command().error(
                ErrorKind::ArgumentConflict,
                "--json is only available for one-shot output",
            ));
        }
        if self.hook_context
            && (self.command.is_some()
                || self.full
                || self.json
                || self.image.is_some()
                || self.no_image
                || self.protocol.is_some())
        {
            return Err(Self::command().error(
                ErrorKind::ArgumentConflict,
                "--hook-context is only available for compact internal one-shot output",
            ));
        }
        if matches!(self.command, Some(Command::Hook { .. }))
            && (self.full
                || self.json
                || self.config.is_some()
                || self.image.is_some()
                || self.no_image
                || self.protocol.is_some()
                || self.color.is_some())
        {
            return Err(Self::command().error(
                ErrorKind::ArgumentConflict,
                "display options are not available for hook generation",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_shot_defaults_are_stable() {
        let cli = Cli::try_parse_from(["hoshino"]).unwrap();
        assert!(!cli.full);
        assert_eq!(cli.color, None);
        assert_eq!(cli.protocol, None);
        assert_eq!(cli.command, None);
    }

    #[test]
    fn incompatible_modes_are_rejected() {
        assert!(Cli::try_parse_from(["hoshino", "--image", "a.png", "--no-image"]).is_err());
        assert!(Cli::try_parse_from(["hoshino", "live", "--json"]).is_err());
        assert!(Cli::try_parse_from(["hoshino", "hook", "bash", "--full"]).is_err());
        assert!(Cli::try_parse_from(["hoshino", "live", "--hook-context"]).is_err());
        assert!(Cli::try_parse_from(["hoshino", "hook", "bash", "--hook-context"]).is_err());
        assert!(Cli::try_parse_from(["hoshino", "--hook-context", "--full"]).is_err());
        assert!(Cli::try_parse_from(["hoshino", "--hook-context", "--json"]).is_err());
    }

    #[test]
    fn hidden_hook_context_is_parseable_but_not_public_help() {
        assert!(
            Cli::try_parse_from(["hoshino", "--hook-context"])
                .unwrap()
                .hook_context
        );
        let help = Cli::command().render_long_help().to_string();
        assert!(!help.contains("hook-context"));
    }
}
