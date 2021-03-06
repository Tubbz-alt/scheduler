#[macro_use]
extern crate err_derive;
#[macro_use]
extern crate serde_derive;

mod config;

use self::config::{Config, Rule};
use ::pidwatcher::PidWatcher;
use ::scheduler::*;
use std::{
    ffi::OsStr,
    fs,
    path::Path,
    process::exit,
    thread,
    time::{Duration, SystemTime},
};

const DEFAULT_CONFIG_PATH: &str = "/etc/default/process-scheduler/rules.toml";
const USER_CONFIG_PATH: &str = "/etc/process-scheduler/rules.toml";
const RULES: &str = "rules.toml";
const POLL_RATE_MS: u64 = 1000;

fn main() {
    if unsafe { libc::geteuid() != 0 } {
        eprintln!("daemon must be run as root");
        exit(1);
    }

    // Launch daemon with a low priority.
    Process::current().set_priority(19);

    let mut config = load_config();
    let users = unsafe { users::all_users().collect::<Vec<_>>() };

    // Keeps track of what PIDs have been spawned over time.
    let mut watcher = PidWatcher::default();

    loop {
        // Scan for new and removed PIDs, and act on the new PIDs.
        watcher.scan(|new_processes| {
            new_processes.iter().for_each(|process| {
                if let Ok(exe) = process.exe() {
                    if let Some(name) = exe.file_name().and_then(OsStr::to_str) {
                        let owner = process.owner;
                        let process = Process::new((process.pid() as u32).into());

                        config
                            .find_rules(name, owner, &users)
                            .for_each(move |rule| apply_rule(process, rule, name));
                    }
                }
            });
        });

        thread::sleep(Duration::from_millis(POLL_RATE_MS));

        let update_config = config.mtime.as_mut().map_or(false, |reference_time| {
            let current_time = get_config_time();
            let requires_update = *reference_time < current_time;

            if requires_update {
                *reference_time = current_time;
            }

            requires_update
        });

        if update_config {
            println!("reloading config");
            config = load_config();
            watcher.reset();
        }
    }
}

/// Attempts to load the configuration for the daemon, fallbacking back to a default if a
/// user-defined configuration file was not define.
fn load_config() -> Config {
    if Path::new(USER_CONFIG_PATH).exists() {
        eprintln!("loading user config");
        match Config::from_path(USER_CONFIG_PATH) {
            Ok(mut config) => {
                config.mtime = Some(get_config_time());
                return config;
            }
            Err(why) => {
                eprintln!("failed to load config at {}: {}", USER_CONFIG_PATH, why);
            }
        }
    }

    let path = if Path::new(DEFAULT_CONFIG_PATH).exists() {
        eprintln!("loading default config");
        DEFAULT_CONFIG_PATH
    } else {
        eprintln!("loading fallback config");
        RULES
    };

    Config::from_path(path).expect("default config not found")
}

fn get_config_time() -> SystemTime {
    fs::metadata(USER_CONFIG_PATH)
        .expect("failed to get metadata from config")
        .modified()
        .expect("failed to get mtime from config")
}

/// Applies every available parameter of a rule to a process.
fn apply_rule<P: CpuPriority + Scheduling>(process: P, rule: &Rule, name: &str) {
    if let Some(priority) = rule.priority {
        println!("Setting {} priority to {}", name, priority);
        process.set_priority(priority);
    }

    if let Some(policy) = rule.policy {
        println!("Setting {} policy to {:?}", name, policy);
        let _ = process.set_scheduler(policy, Parameters::default());
    }
}
