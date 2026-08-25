//! Terminal output: step lines, outcomes, colors.

use std::io::IsTerminal;

use owo_colors::OwoColorize;

fn color() -> bool {
    std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal()
}

pub fn step(host: &str, text: &str) {
    if color() {
        println!("  {} {text}", host.bold().cyan());
    } else {
        println!("  {host} {text}");
    }
}

pub fn note(text: &str) {
    println!("{text}");
}

pub fn ok(text: &str) {
    if color() {
        println!("{}", text.green());
    } else {
        println!("{text}");
    }
}

pub fn fail(text: &str) {
    if color() {
        eprintln!("{}", text.red().bold());
    } else {
        eprintln!("{text}");
    }
}
