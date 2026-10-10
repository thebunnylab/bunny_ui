//! The top of the program: the command table, the help, and the one
//! place a failure is printed and turned into an exit code.

use std::process::ExitCode;

use crate::args::{self, Matches, Opt};
use crate::commands;
use crate::error::{Error, Result};
use crate::term;

/// One `bunny` command.
pub struct Command {
    pub name: &'static str,
    pub summary: &'static str,
    pub usage: &'static str,
    pub about: &'static str,
    pub options: &'static [Opt],
    pub run: fn(&Matches) -> Result<()>,
}

pub const COMMANDS: &[Command] = &[
    Command {
        name: "new",
        summary: commands::new::SUMMARY,
        usage: commands::new::USAGE,
        about: commands::new::ABOUT,
        options: commands::new::OPTIONS,
        run: commands::new::run,
    },
    Command {
        name: "run",
        summary: commands::run::SUMMARY,
        usage: commands::run::USAGE,
        about: commands::run::ABOUT,
        options: commands::run::OPTIONS,
        run: commands::run::run,
    },
    Command {
        name: "doctor",
        summary: commands::doctor::SUMMARY,
        usage: commands::doctor::USAGE,
        about: commands::doctor::ABOUT,
        options: commands::doctor::OPTIONS,
        run: commands::doctor::run,
    },
    Command {
        name: "devices",
        summary: commands::devices::DEVICES_SUMMARY,
        usage: commands::devices::DEVICES_USAGE,
        about: commands::devices::DEVICES_ABOUT,
        options: commands::devices::DEVICES_OPTIONS,
        run: commands::devices::devices,
    },
    Command {
        name: "emulators",
        summary: commands::devices::EMULATORS_SUMMARY,
        usage: commands::devices::EMULATORS_USAGE,
        about: commands::devices::EMULATORS_ABOUT,
        options: commands::devices::EMULATORS_OPTIONS,
        run: commands::devices::emulators,
    },
];

/// The program: the arguments in, an exit code out — 0, 1 for a failure,
/// 2 for a command line that could not be read.
pub fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match dispatch(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            term::print_error(&error.message, error.hint.as_deref());
            ExitCode::from(error.code)
        }
    }
}

/// Runs the command the arguments name.
pub fn dispatch(args: &[String]) -> Result<()> {
    let Some(first) = args.first() else {
        print!("{}", help());
        return Ok(());
    };
    match first.as_str() {
        "-h" | "--help" => {
            print!("{}", help());
            Ok(())
        }
        "-V" | "--version" => {
            println!("bunny {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "help" => match args.get(1) {
            None => {
                print!("{}", help());
                Ok(())
            }
            Some(name) => {
                print!("{}", command_help(find(name)?));
                Ok(())
            }
        },
        name => {
            let command = find(name)?;
            let matches = args::parse(command.options, &args[1..])?;
            if matches.flag("help") {
                print!("{}", command_help(command));
                return Ok(());
            }
            (command.run)(&matches)
        }
    }
}

fn find(name: &str) -> Result<&'static Command> {
    if let Some(command) = COMMANDS.iter().find(|command| command.name == name) {
        return Ok(command);
    }
    let error = Error::usage(format!("unknown command `{name}`"));
    Err(match args::nearest(name, COMMANDS.iter().map(|command| command.name)) {
        Some(near) => error.hint(format!("did you mean `bunny {near}`?")),
        None => error.hint("`bunny --help` lists the commands"),
    })
}

/// The program's help.
pub fn help() -> String {
    let width = COMMANDS.iter().map(|command| command.name.len()).max().unwrap_or(0).max("help".len());
    let mut out = format!(
        "{} {} — create and run bunny-ui apps\n\n{} bunny <COMMAND> [OPTIONS]\n\n{}\n",
        term::bold("bunny"),
        env!("CARGO_PKG_VERSION"),
        term::bold("Usage:"),
        term::bold("Commands:")
    );
    for command in COMMANDS {
        out.push_str(&format!("  {}  {}\n", term::cyan(&format!("{:width$}", command.name)), command.summary));
    }
    out.push_str(&format!("  {}  Print a command's help\n", term::cyan(&format!("{:width$}", "help"))));
    out.push_str(&format!(
        "\n{}\n  -h, --help     Print this help\n  -V, --version  Print the version\n\n\
         `bunny help <COMMAND>` prints a command's options.\n",
        term::bold("Options:")
    ));
    out
}

/// A command's help.
pub fn command_help(command: &Command) -> String {
    format!(
        "{}\n\n{} {}\n\n{}\n\n{}\n{}",
        command.summary,
        term::bold("Usage:"),
        command.usage,
        command.about,
        term::bold("Options:"),
        args::help_block(command.options)
    )
}
