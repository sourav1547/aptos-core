mod cargo;
mod common;

use cargo::Cargo;
use clap::{Args, Parser, Subcommand};
use common::{ChangeSet, SelectedPackageArgs};
use guppy::graph::DependencyDirection;

#[derive(Clone, Subcommand, Debug)]
enum Command {
    ChangedSince(CommonArgs),
    Check(CommonArgs),
    Fmt(CommonArgs),
    Nextest(CommonArgs),
    Test(CommonArgs),
    Xclippy(CommonArgs),
}

impl Command {
    fn command(&self) -> &'static str {
        match self {
            Command::Check(_) => "check",
            Command::Fmt(_) => "fmt",
            Command::Nextest(_) => "nextest",
            Command::Test(_) => "test",
            Command::Xclippy(_) => "xclippy",
            _ => unimplemented!(),
        }
    }

    fn command_args(&self) -> &CommonArgs {
        match self {
            Command::Check(args) => args,
            Command::Fmt(args) => args,
            Command::Nextest(args) => args,
            Command::Test(args) => args,
            Command::Xclippy(args) => args,
            _ => unimplemented!(),
        }
    }
}

#[derive(Args, Clone, Debug)]
#[command(disable_help_flag = true)]
struct CommonArgs {
    #[command(flatten)]
    package_args: SelectedPackageArgs,
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

impl CommonArgs {
    fn args(&self) -> (Vec<String>, Vec<String>) {
        if let Some(index) = self.args.iter().position(|arg| arg == "--") {
            let (left, right) = self.args.split_at(index);
            (left.to_vec(), right[1..].to_vec())
        } else {
            (self.args.clone(), vec![])
        }
    }
}

#[derive(Parser, Debug)]
#[clap(name = "x", author, version)]
pub struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

impl Cli {
    pub fn execute(&self) -> anyhow::Result<()> {
        let (mut direct_args, push_through_args) = self.cmd.command_args().args();

        let packages = if self.cmd.command_args().package_args.package.is_empty() {
            let change_set = ChangeSet::init()?;
            let determinator_set = change_set.determine_changed_packages();

            // determinator_set.affected_set contains the workspace packages directly or indirectly affected
            // by the change.
            let mut ret = vec![];
            for package in determinator_set
                .affected_set
                .packages(DependencyDirection::Forward)
            {
                println!("affected: {}", package.name());
                ret.push(package.name().into())
            }
            ret
        } else {
            self.cmd.command_args().package_args.package.clone()
        };

        for p in packages {
            direct_args.push("-p".into());
            direct_args.push(p);
        }

        Cargo::command(self.cmd.command())
            .args(direct_args)
            .pass_through(push_through_args)
            .run();
        Ok(())
    }
}

#[derive(Parser, Debug)]
pub struct TestCommand {}

impl TestCommand {
    pub fn execute(&self) -> anyhow::Result<()> {
        let change_set = ChangeSet::init()?;
        let determinator_set = change_set.determine_changed_packages();

        // determinator_set.affected_set contains the workspace packages directly or indirectly affected
        // by the change.
        for package in determinator_set
            .affected_set
            .packages(DependencyDirection::Forward)
        {
            println!("affected: {}", package.name());
        }

        Ok(())
    }
}
