use colored::*;
use lazy_static::lazy_static;
use reqwest;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use tokio;

lazy_static! {
    static ref PROTON_VERSIONS: HashMap<u32, &'static str> = {
        let mut m = HashMap::new();
        m.insert(1493710, "Proton Experimental");
        m.insert(2805730, "Proton 9.0");
        m
    };
}

#[derive(Debug, Serialize, Deserialize)]
struct SteamAppInfo {
    appid: u32,
    name: String,
}

#[derive(Debug, PartialEq)]
struct CompatData {
    path: PathBuf,
    app_id: u32,
}

#[derive(Debug)]
struct SteamLibrary {
    path: PathBuf,
    installed_apps: HashSet<u32>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let home = std::env::var("HOME")?;

    let flatpak_path = Path::new(&home).join(".var/app/com.valvesoftware.Steam/.local/share/Steam");
    let regular_path = Path::new(&home).join(".local/share/Steam");

    let steam_path = if flatpak_path.exists() {
        flatpak_path
    } else {
        regular_path
    };

    println!("{}", "Steam Compatdata Analyzer".bold().green());
    println!(
        "INFO: Using Steam path: {}\n",
        steam_path.display().to_string().blue()
    );

    let libraries = get_steam_libraries(&steam_path)?;
    println!("INFO: Found {} Steam libraries.", libraries.len());

    let mut all_installed_apps: HashSet<u32> = HashSet::new();
    for library in &libraries {
        println!("INFO: Processing library at: {}", library.path.display());
        all_installed_apps.extend(&library.installed_apps);
    }

    println!("{}", "Analyzing compatdata directories...".bold());
    println!("{}", "===================================".bold());

    let mut proton_versions_found = HashSet::new();

    // Scan compatdata from all libraries.
    let mut all_compatdata = Vec::new();
    for library in &libraries {
        all_compatdata.extend(scan_compatdata_dirs(&library.path));
    }

    for entry in all_compatdata {
        let app_id = entry.app_id;
        let is_installed = all_installed_apps.contains(&app_id);
        let is_proton = PROTON_VERSIONS.contains_key(&app_id);

        if is_proton {
            proton_versions_found.insert(app_id);
        }

        match fetch_app_info(app_id).await {
            Some((success, name)) => {
                let status = if is_installed {
                    "INSTALLED".green()
                } else {
                    "NOT INSTALLED".yellow()
                };

                let app_status = if success {
                    if is_proton {
                        name.cyan()
                    } else {
                        name.white()
                    }
                } else {
                    "Unknown Application".red()
                };

                println!(
                    "AppID {:6} | {:<50} | {}",
                    app_id.to_string().blue(),
                    app_status,
                    status
                );
            }
            None => {
                println!(
                    "AppID {:6} | {:<50} | {}",
                    app_id.to_string().blue(),
                    "Failed to fetch app info".red(),
                    if is_installed {
                        "INSTALLED".green()
                    } else {
                        "NOT INSTALLED".yellow()
                    }
                );
            }
        }

        // Add a small delay to avoid hitting Steam's rate limits.
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    }

    if !proton_versions_found.is_empty() {
        println!("\n{}", "Proton Versions Found:".bold().cyan());
        println!("{}", "======================".bold());

        for app_id in proton_versions_found {
            if let Some(name) = PROTON_VERSIONS.get(&app_id) {
                println!("{:<15} | AppID: {}", name.cyan(), app_id.to_string().blue());
            }
        }
    }

    println!("\n{}", "Analysis complete!".bold().green());
    Ok(())
}

fn get_steam_libraries(steam_path: &Path) -> Result<Vec<SteamLibrary>, Box<dyn std::error::Error>> {
    let mut libraries = Vec::new();

    // Add the main Steam library.
    libraries.push(SteamLibrary {
        path: steam_path.to_path_buf(),
        installed_apps: parse_installed_apps(&steam_path.join("steamapps/libraryfolders.vdf"))?,
    });

    // Parse libraryfolders.vdf to find additional libraries.
    let content = fs::read_to_string(steam_path.join("steamapps/libraryfolders.vdf"))?;
    let mut current_path = None;

    for line in content.lines() {
        let trimmed = line.trim();

        // Look for "path" entries.
        if trimmed.starts_with("\"path\"") {
            if let Some(path) = trimmed.split('"').nth(3) {
                current_path = Some(PathBuf::from(path));
            }
        }

        // When we find a path and reach its closing brace, add it as a library.
        if trimmed == "}" && current_path.is_some() {
            let path = current_path.take().unwrap();

            if path.exists() && path != steam_path {
                libraries.push(SteamLibrary {
                    installed_apps: parse_installed_apps(
                        &path.join("steamapps/libraryfolders.vdf"),
                    )
                    .unwrap_or_else(|_| HashSet::new()),
                    path,
                });
            }
        }
    }

    Ok(libraries)
}

fn parse_installed_apps(config_path: &Path) -> Result<HashSet<u32>, Box<dyn std::error::Error>> {
    let mut installed_apps = HashSet::new();
    let mut in_apps_section = false;

    if let Ok(content) = fs::read_to_string(config_path) {
        for line in content.lines() {
            let trimmed_line = line.trim();

            if trimmed_line == "\"apps\"" {
                in_apps_section = true;
                continue;
            }

            if in_apps_section && trimmed_line == "}" {
                in_apps_section = false;
                continue;
            }

            if in_apps_section && trimmed_line.starts_with('"') {
                if let Some(app_id_str) = trimmed_line.split('"').nth(1) {
                    if let Ok(app_id) = app_id_str.parse::<u32>() {
                        installed_apps.insert(app_id);
                    }
                }
            }
        }
    }

    Ok(installed_apps)
}

async fn fetch_app_info(app_id: u32) -> Option<(bool, String)> {
    // First check if this is a known Proton version.
    if let Some(proton_name) = PROTON_VERSIONS.get(&app_id) {
        return Some((true, proton_name.to_string()));
    }

    let url = format!(
        "https://store.steampowered.com/api/appdetails?appids={}",
        app_id
    );

    println!("Fetched app info for {} and got {}.", app_id, url);

    match reqwest::get(&url).await {
        Ok(response) => {
            if let Ok(text) = response.text().await {
                if let Ok(json) = serde_json::from_str::<Value>(&text) {
                    if let Some(app_data) = json.get(&app_id.to_string()) {
                        let success = app_data["success"].as_bool().unwrap_or(false);
                        let name = if success {
                            app_data["data"]["name"]
                                .as_str()
                                .unwrap_or("Unknown")
                                .to_string()
                        } else {
                            "Unknown Application".to_string()
                        };

                        return Some((success, name));
                    }
                }
            }

            None
        }
        Err(_) => None,
    }
}

fn scan_compatdata_dirs(steam_path: &Path) -> Vec<CompatData> {
    let compatdata_path = steam_path.join("steamapps/compatdata");
    let mut compat_entries = Vec::new();

    if let Ok(entries) = fs::read_dir(&compatdata_path) {
        for entry in entries.filter_map(Result::ok) {
            if let Some(dir_name) = entry.file_name().to_str() {
                if dir_name != "0" {
                    if let Ok(app_id) = dir_name.parse::<u32>() {
                        compat_entries.push(CompatData {
                            path: entry.path(),
                            app_id,
                        })
                    }
                }
            }
        }
    }

    compat_entries
}
