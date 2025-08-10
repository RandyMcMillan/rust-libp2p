use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::Path;

fn main() -> io::Result<()> {
    let mut current_dir = std::env::current_dir()?;

    loop {
        let git_dir = current_dir.join(".git/logs/refs/heads");

        if git_dir.exists() && git_dir.is_dir() {
            process_logs_refs(&git_dir)?;
            break;
        }

        if let Some(parent) = current_dir.parent() {
            current_dir = parent.to_path_buf();
        } else {
            eprintln!("Error: .git directory not found in any parent directories.");
            break;
        }
    }

    if let Ok(entries) = fs::read_dir(current_dir) {
        for entry in entries {
            if let Ok(entry) = entry {
                let path = entry.path();

                if path.is_file() {
                    if let Err(e) = process_log_file(&path) {
                        eprintln!("Error processing {:?}: {}", path, e);
                    }
                }
            }
        }
    }

    Ok(())
}

fn process_logs_refs(logs_ref_dir: &Path) -> io::Result<()> {
    if let Ok(entries) = fs::read_dir(logs_ref_dir) {
        for entry in entries {
            if let Ok(entry) = entry {
                let path = entry.path();

                if path.is_file() {
                    if let Err(e) = process_log_file(&path) {
                        eprintln!("Error processing {:?}: {}", path, e);
                    }
                }
            }
        }
    }

    Ok(())
}

fn process_log_file(file_path: &Path) -> io::Result<()> {
    let file = File::open(file_path)?;
    let reader = BufReader::new(file);

    println!("Log file: {:?}", file_path);

    for line_result in reader.lines() {
        let line = line_result?;
        let log_entry = parse_log_entry(&line);
        println!("{:?}", log_entry);
    }
    println!();
    Ok(())
}

fn parse_log_entry(line: &str) -> Option<LogEntry> {
    let parts: Vec<&str> = line.split_whitespace().collect();

    if parts.len() < 3 {
        return None;
    }

    let old_commit = parts[0].to_string();
    let new_commit = parts[1].to_string();
    let author_name = parts[2].split('<').next().unwrap_or("").to_string();
    let author_email = parts[2]
        .split('<')
        .nth(1)
        .and_then(|s| s.split('>').next())
        .unwrap_or("")
        .to_string();

    let timestamp_str = parts.get(3).unwrap_or(&"").to_string();
    let timezone_str = parts.get(4).unwrap_or(&"").to_string();

    let message_start = parts
        .iter()
        .position(|&p| p.starts_with("message:"))
        .unwrap_or(parts.len());
    let message = parts[message_start..]
        .join(" ")
        .replace("message:", "")
        .trim()
        .to_string();

    Some(LogEntry {
        old_commit,
        new_commit,
        author_name,
        author_email,
        timestamp: timestamp_str,
        timezone: timezone_str,
        message,
    })
}

#[derive(Debug)]
struct LogEntry {
    old_commit: String,
    new_commit: String,
    author_name: String,
    author_email: String,
    timestamp: String,
    timezone: String,
    message: String,
}
