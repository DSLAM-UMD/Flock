// Copyright (c) 2020-present, UMD Database Group.
//
// This program is free software: you can use, redistribute, and/or modify
// it under the terms of the GNU Affero General Public License, version 3
// or later ("AGPL"), as published by the Free Software Foundation.
//
// This program is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or
// FITNESS FOR A PARTICULAR PURPOSE.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program. If not, see <http://www.gnu.org/licenses/>.

use crate::arch;
use crate::args;
use crate::fsql;
use crate::lambda;
use crate::nexmark;
use crate::s3;
use crate::ysb;
use anyhow::Context as _;
use anyhow::{anyhow, Result};
use clap::{crate_version, App, AppSettings};
use log::warn;

#[tokio::main]
pub async fn main() -> Result<()> {
    // Command line arg parsing and configuration.
    let app_cli = App::new("Flock")
        .version(crate_version!())
        .about("Command Line Interactive Contoller for Flock")
        .version(crate_version!())
        .setting(AppSettings::SubcommandRequired)
        .setting(AppSettings::PropagateVersion)
        .author("UMD Database Group")
        .args(&args::get_args())
        .subcommand(nexmark::command_args())
        .subcommand(ysb::command_args())
        .subcommand(s3::command_args())
        .subcommand(lambda::command_args())
        .subcommand(arch::command_args())
        .subcommand(fsql::command_args());

    let global_matches = app_cli.get_matches();
    let (command, matches) = match global_matches.subcommand() {
        Some((command, matches)) => (command, matches),
        None => unreachable!(),
    };

    args::get_logging(&global_matches, matches)?.init();

    match command {
        "nexmark" => nexmark::command(matches),
        "ysb" => ysb::command(matches),
        "s3" => s3::command(matches),
        "lambda" => lambda::command(matches),
        "fsql" => fsql::command(matches),
        "arch" => arch::command(matches),
        _ => {
            warn!("{} command is not implemented", command);
            Ok(())
        }
    }
    .with_context(|| anyhow!("{} command failed", command))?;

    Ok(())
}
