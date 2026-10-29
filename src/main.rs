//! RhubTui — USB 设备查看器与移动盘挂载管理。

#![recursion_limit = "256"]

mod app;
mod cli;
mod config;
mod desc;
mod model;
mod mount;
mod ops;
mod probe;
mod scan;
#[cfg(test)]
mod testing;
mod theme;
mod ui;
mod view;

use std::process::ExitCode;

use clap::Parser;

use cli::Cli;
use config::Config;

fn main() -> ExitCode {
    // 管道输出被截断时安静退出，而不是 panic。
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let cli = Cli::parse();

    let config = match Config::load() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("读取配置失败：{error:#}");
            return ExitCode::FAILURE;
        }
    };

    let result = match cli.command {
        None => app::run(config),
        Some(command) => cli::run(command, cli.json),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("错误：{error:#}");
            ExitCode::FAILURE
        }
    }
}
