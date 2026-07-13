use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

static DEFAULT_CONFIG: &str = include_str!("../config.json");

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Config {
    #[serde(default)]
    game_path: Option<String>,
    #[serde(default)]
    manifest_url: String,
    #[serde(default)]
    addons_url: String,
    #[serde(default)]
    server_address: String,
    #[serde(default)]
    account_url: String,
    #[serde(default)]
    app_name: String,
    #[serde(default)]
    app_tagline: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Downloads {
    wotlk: Option<String>,
    tbc: Option<String>,
    classic: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct AddonEntry {
    title: Option<String>,
    name: Option<String>,
    description: Option<String>,
    author: Option<String>,
    image: Option<String>,
    downloads: Option<Downloads>,
    #[serde(alias = "downloadUrl")]
    download_url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct InstalledAddon {
    name: String,
    title: String,
    author: Option<String>,
    version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClientManifest {
    version: Option<String>,
    base_url: Option<String>,
    files: Vec<ManifestFile>,
}

#[derive(Debug, Deserialize)]
struct ManifestFile {
    path: String,
    #[serde(alias = "hash")]
    sha256: String,
    #[serde(default)]
    size: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct ClientStatus {
    phase: String,
    manifest_total: u64,
    files_to_sync: u64,
    installed_size: u64,
}

#[derive(Debug, Serialize, Clone)]
struct ClientSyncProgress {
    downloaded: u64,
    total: u64,
    speed: String,
    phase: String,
    message: String,
}

struct AppState {
    config: Mutex<Config>,
    config_path: PathBuf,
    download_paused: AtomicBool,
}

fn config_dir(app: &tauri::AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn load_config(path: &PathBuf, default: &Config) -> Config {
    if path.exists() {
        if let Ok(content) = std::fs::read_to_string(path) {
            if let Ok(mut c) = serde_json::from_str::<Config>(&content) {
                if c.manifest_url.is_empty() { c.manifest_url = default.manifest_url.clone(); }
                if c.addons_url.is_empty() { c.addons_url = default.addons_url.clone(); }
                if c.server_address.is_empty() { c.server_address = default.server_address.clone(); }
                if c.account_url.is_empty() { c.account_url = default.account_url.clone(); }
                return c;
            }
        }
    }
    default.clone()
}

fn save_config(path: &PathBuf, config: &Config) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(content) = serde_json::to_string_pretty(config) {
        let _ = std::fs::write(path, content);
    }
}

fn game_dir(config: &Config) -> Option<PathBuf> {
    let path = config.game_path.as_ref()?;
    let p = PathBuf::from(path);
    if p.exists() {
        Some(if p.is_file() {
            p.parent().map(|x| x.to_path_buf()).unwrap_or(p)
        } else {
            p
        })
    } else {
        None
    }
}

fn find_wow_exe(dir: &PathBuf) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if name == "wow.exe" || name == "wowclassic.exe" || name == "world of warcraft.exe" {
            return Some(entry.path());
        }
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            if let Some(found) = find_wow_exe(&entry.path()) {
                return Some(found);
            }
        }
    }
    None
}

fn realmlist_paths(game_dir: &PathBuf) -> Vec<PathBuf> {
    vec![
        game_dir.join("realmlist.wtf"),
        game_dir.join("Data").join("enUS").join("realmlist.wtf"),
        game_dir.join("Data").join("enGB").join("realmlist.wtf"),
    ]
}

fn ensure_realmlist(game_dir: &PathBuf, server_addr: &str) {
    let content = format!("set realmlist {}\n", server_addr);
    for p in &realmlist_paths(game_dir) {
        if p.exists() { return; }
    }
    let fallback = game_dir.join("Data").join("enUS");
    let _ = std::fs::create_dir_all(&fallback);
    let _ = std::fs::write(fallback.join("realmlist.wtf"), &content);
}

fn compute_sha256(path: &PathBuf) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|e| e.to_string())?;
    Ok(hex::encode(hasher.finalize()))
}

fn clients_dir(app: &tauri::AppHandle) -> PathBuf {
    config_dir(app).join("clients")
}

fn emit<E: serde::Serialize + Clone>(app: &tauri::AppHandle, event: &str, payload: E) -> Result<(), String> {
    app.emit(event, payload).map_err(|e| e.to_string())
}

fn installed_size(dir: &PathBuf) -> u64 {
    fn walk(path: &PathBuf) -> u64 {
        let mut size = 0u64;
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                if let Ok(meta) = entry.metadata() {
                    if meta.is_dir() {
                        size += walk(&entry.path());
                    } else {
                        size += meta.len();
                    }
                }
            }
        }
        size
    }
    walk(dir)
}

// ── Commands ────────────────────────────────────────────────

#[tauri::command]
fn get_config(state: State<AppState>) -> Config {
    state.config.lock().unwrap().clone()
}

#[tauri::command]
fn save_settings(state: State<AppState>, game_path: Option<String>) {
    let path = state.config_path.clone();
    let mut config = state.config.lock().unwrap();
    config.game_path = game_path;
    save_config(&path, &config);
}

#[tauri::command]
async fn get_client_status(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<ClientStatus, String> {
    let config = state.config.lock().unwrap().clone();
    let dir = clients_dir(&app);

    let manifest: ClientManifest = reqwest::get(&config.manifest_url)
        .await
        .map_err(|e| format!("Failed to fetch manifest: {}", e))?
        .json()
        .await
        .map_err(|e| format!("Invalid manifest: {}", e))?;

    let total = manifest.files.len() as u64;
    let mut missing = 0u64;

    for file in &manifest.files {
        let local = dir.join(&file.path);
        if !local.exists() {
            missing += 1;
        } else {
            match compute_sha256(&local) {
                Ok(h) if h != file.sha256 => missing += 1,
                _ => {}
            }
        }
    }

    let isize = installed_size(&dir);
    let phase = if !dir.exists() || missing == total || find_wow_exe(&dir).is_none() {
        "not_installed"
    } else if missing > 0 {
        "needs_update"
    } else {
        "ready"
    };

    Ok(ClientStatus {
        phase: phase.into(),
        manifest_total: total,
        files_to_sync: missing,
        installed_size: isize,
    })
}

#[tauri::command]
fn launch_game(state: State<AppState>) -> Result<(), String> {
    let config = state.config.lock().unwrap();
    let dir = game_dir(&config).ok_or("Game client not installed. Click Install to download it.")?;
    let exe = find_wow_exe(&dir).ok_or("Could not find WoW executable")?;
    ensure_realmlist(&dir, &config.server_address);

    let child = if cfg!(target_os = "linux") {
        Command::new("wine").arg(&exe)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    } else {
        Command::new(&exe)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    };

    match child {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("Failed to launch: {}", e)),
    }
}

#[tauri::command]
fn clear_cache(state: State<AppState>) -> Result<(), String> {
    let config = state.config.lock().unwrap();
    let dir = game_dir(&config).ok_or("Game client not installed")?;

    for d in [dir.join("WDB"), dir.join("Cache")] {
        if d.exists() { std::fs::remove_dir_all(&d).map_err(|e| e.to_string())?; }
    }
    Ok(())
}

#[tauri::command]
fn list_installed_addons(state: State<AppState>) -> Vec<InstalledAddon> {
    let config = state.config.lock().unwrap();
    let dir = match game_dir(&config) {
        Some(d) => d,
        None => return vec![],
    };
    let addons_dir = dir.join("Interface").join("AddOns");
    if !addons_dir.exists() { return vec![]; }

    let mut addons = vec![];
    let entries = match std::fs::read_dir(&addons_dir) { Ok(e) => e, Err(_) => return vec![] };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("Blizzard_") { continue; }
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) { continue; }

        let toc = entry.path().join(format!("{}.toc", &name));
        let toc = if toc.exists() { toc } else {
            let mut found = None;
            if let Ok(files) = std::fs::read_dir(entry.path()) {
                for f in files.flatten() {
                    if f.path().extension().map(|e| e == "toc").unwrap_or(false) {
                        found = Some(f.path()); break;
                    }
                }
            }
            match found { Some(p) => p, None => continue }
        };

        let content = match std::fs::read_to_string(&toc) { Ok(c) => c, Err(_) => continue };

        let title = content.lines()
            .find(|l| l.starts_with("## Title:"))
            .map(|l| l.trim_start_matches("## Title:").trim().to_string())
            .unwrap_or_else(|| name.clone());

        let author = content.lines()
            .find(|l| l.starts_with("## Author:"))
            .map(|l| l.trim_start_matches("## Author:").trim().to_string());

        let version = content.lines()
            .find(|l| l.starts_with("## Version:"))
            .map(|l| l.trim_start_matches("## Version:").trim().to_string());

        addons.push(InstalledAddon { name, title, author, version });
    }
    addons
}

#[tauri::command]
async fn fetch_addon_list(state: State<'_, AppState>) -> Result<Vec<AddonEntry>, String> {
    let config = state.config.lock().unwrap().clone();
    if config.addons_url.is_empty() { return Ok(vec![]); }

    let resp = reqwest::get(&config.addons_url).await
        .map_err(|e| format!("Failed to fetch addon list: {}", e))?;
    let text = resp.text().await
        .map_err(|e| format!("Failed to read response: {}", e))?;

    if let Ok(entries) = serde_json::from_str::<Vec<AddonEntry>>(&text) {
        return Ok(entries);
    }

    if let Ok(wrapper) = serde_json::from_str::<serde_json::Value>(&text) {
        if let Some(arr) = wrapper.get("addons").and_then(|v| v.as_array()) {
            let entries: Vec<AddonEntry> = arr.iter().map(|item| {
                let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let url = item.get("url").and_then(|v| v.as_str()).map(|s| s.to_string());
                AddonEntry {
                    title: Some(name.trim_end_matches(".zip").to_string()),
                    name: None, description: None, author: None, image: None,
                    downloads: Some(Downloads { wotlk: url.clone(), tbc: None, classic: None }),
                    download_url: url,
                }
            }).collect();
            return Ok(entries);
        }
    }

    Err("Unrecognized addon list format".into())
}

#[tauri::command]
async fn install_addon(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    addon: AddonEntry,
    expansion: String,
) -> Result<(), String> {
    let download_url = addon.download_url.as_deref().map(|s| s.to_string());
    let download_url = match download_url {
        Some(u) => u,
        None => {
            let dl = addon.downloads.as_ref().and_then(|d| match expansion.as_str() {
                "wotlk" => d.wotlk.clone(),
                "tbc" => d.tbc.clone(),
                "classic" => d.classic.clone(),
                _ => None,
            });
            dl.ok_or("No download URL for this addon")?
        }
    };

    let (addons_dir, temp_dir) = {
        let config = state.config.lock().unwrap();
        let dir = game_dir(&config).ok_or("Game client not installed")?;
        let a_dir = dir.join("Interface").join("AddOns");
        std::fs::create_dir_all(&a_dir).map_err(|e| e.to_string())?;
        let t_dir = app.path().app_data_dir().unwrap_or_else(|_| PathBuf::from("."));
        std::fs::create_dir_all(&t_dir).map_err(|e| e.to_string())?;
        (a_dir, t_dir)
    };

    let zip_path = temp_dir.join(format!("addon_{}.zip", rand_id()));
    let response = reqwest::get(&download_url).await.map_err(|e| format!("Download failed: {}", e))?;
    let bytes = response.bytes().await.map_err(|e| format!("Download failed: {}", e))?;

    let mut file = std::fs::File::create(&zip_path).map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    drop(file);

    let zip_file = std::fs::File::open(&zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(zip_file).map_err(|e| e.to_string())?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let out_path = entry.mangled_name();
        let full_path = addons_dir.join(&out_path);
        if entry.is_dir() {
            std::fs::create_dir_all(&full_path).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = full_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut outfile = std::fs::File::create(&full_path).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut outfile).map_err(|e| e.to_string())?;
        }
    }
    let _ = std::fs::remove_file(&zip_path);
    Ok(())
}

fn rand_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    format!("{:x}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos())
}

#[tauri::command]
fn delete_addon(state: State<AppState>, name: String) -> Result<(), String> {
    let config = state.config.lock().unwrap();
    let dir = game_dir(&config).ok_or("Game client not installed")?;
    let addon_path = dir.join("Interface").join("AddOns").join(&name);
    if addon_path.exists() {
        std::fs::remove_dir_all(&addon_path).map_err(|e| e.to_string())
    } else {
        Err(format!("Addon '{}' not found", name))
    }
}

#[tauri::command]
fn select_game_path(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let file = app.dialog().file()
        .add_filter("Game Executable", &["exe"])
        .blocking_pick_file();
    Ok(file.and_then(|f| f.as_path().map(|p| p.to_string_lossy().to_string())))
}

#[tauri::command]
async fn download_client(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let config = state.config.lock().unwrap().clone();
    let dir = clients_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    emit(&app, "client-progress", ClientSyncProgress {
        downloaded: 0, total: 0, speed: "".into(),
        phase: "connecting".into(), message: "Fetching manifest...".into(),
    })?;

    let manifest: ClientManifest = reqwest::get(&config.manifest_url).await
        .map_err(|e| format!("Failed to fetch manifest: {}", e))?
        .json().await
        .map_err(|e| format!("Invalid manifest: {}", e))?;

    let total = manifest.files.len() as u64;
    let base_url = manifest.base_url.unwrap_or_default();

    emit(&app, "client-progress", ClientSyncProgress {
        downloaded: 0, total, speed: "".into(),
        phase: "scanning".into(),
        message: format!("Scanning {} files...", total),
    })?;

    let mut to_download: Vec<&ManifestFile> = Vec::new();
    let mut scanned: u64 = 0;

    for file in &manifest.files {
        let local = dir.join(&file.path);
        if local.is_dir() {
            scanned += 1;
            continue;
        }
        let needs = if local.exists() {
            compute_sha256(&local).map_or(true, |h| h != file.sha256)
        } else { true };
        if needs { to_download.push(file); }
        scanned += 1;
        if scanned % 50 == 0 || scanned == total {
            // cap at total-1 so progress never hits 100% before syncing starts
            let display = if scanned == total { total - 1 } else { scanned };
            emit(&app, "client-progress", ClientSyncProgress {
                downloaded: display, total, speed: "".into(),
                phase: "scanning".into(),
                message: format!("Scanned {}/{} files ({} to sync)", scanned, total, to_download.len()),
            }).ok();
        }
    }

    if to_download.is_empty() {
        let exe = find_wow_exe(&dir).ok_or("Client files exist but no WoW.exe found")?;
        let exe_str = exe.to_string_lossy().to_string();
        ensure_realmlist(&dir, &config.server_address);
        {
            let mut cfg = state.config.lock().unwrap();
            cfg.game_path = Some(exe_str.clone());
            let path = state.config_path.clone();
            save_config(&path, &cfg);
        }
        emit(&app, "client-progress", ClientSyncProgress {
            downloaded: total, total, speed: "".into(),
            phase: "complete".into(), message: "All files up to date!".into(),
        }).ok();
        return Ok(exe_str);
    }

    let dl_total = to_download.len() as u64;
    let offset = total - dl_total; // files already up to date — keeps progress smooth
    let mut dl_current: u64 = 0;
    let start = std::time::Instant::now();
    let mut cumulative_bytes: u64 = 0;

    for file in &to_download {
        // Check pause flag — wait until resumed
        while state.download_paused.load(Ordering::Relaxed) {
            emit(&app, "client-progress", ClientSyncProgress {
                downloaded: offset + dl_current, total,
                speed: String::new(),
                phase: "paused".into(),
                message: format!("Paused — {}/{} files", dl_current, dl_total),
            }).ok();
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        let file_url = if base_url.ends_with('/') {
            format!("{}{}", base_url, file.path)
        } else {
            format!("{}/{}", base_url, file.path)
        };
        let local = dir.join(&file.path);

        if let Some(parent) = local.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        let response = reqwest::get(&file_url).await
            .map_err(|e| format!("Failed: {} - {}", file.path, e))?;
        if !response.status().is_success() {
            return Err(format!("HTTP {} for: {}", response.status(), file.path));
        }
        let bytes = response.bytes().await
            .map_err(|e| format!("Download error: {}", e))?;
        cumulative_bytes += bytes.len() as u64;

        // Atomic write: temp file then rename to prevent partial files on crash
        let tmp = local.with_extension("tmp");
        {
            let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
            f.write_all(&bytes).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&tmp, &local).map_err(|e| e.to_string())?;

        dl_current += 1;
        let elapsed = start.elapsed().as_secs_f64().max(0.001);
        let speed = if cumulative_bytes > 0 {
            format!("{:.1} MB/s", cumulative_bytes as f64 / elapsed / 1_048_576.0)
        } else {
            String::new()
        };
        emit(&app, "client-progress", ClientSyncProgress {
            downloaded: offset + dl_current, total,
            speed,
            phase: "syncing".into(),
            message: format!("Downloading {}/{} files: {}", dl_current, dl_total, file.path),
        }).ok();
    }

    ensure_realmlist(&dir, &config.server_address);

    let exe = find_wow_exe(&dir).ok_or("No WoW.exe found in downloaded client")?;
    let exe_str = exe.to_string_lossy().to_string();
    {
        let mut cfg = state.config.lock().unwrap();
        cfg.game_path = Some(exe_str.clone());
        let path = state.config_path.clone();
        save_config(&path, &cfg);
    }

    emit(&app, "client-progress", ClientSyncProgress {
        downloaded: total, total, speed: "".into(),
        phase: "complete".into(), message: "Game ready!".into(),
    }).ok();

    Ok(exe_str)
}

#[tauri::command]
fn get_realmlist(state: State<AppState>) -> Result<String, String> {
    let config = state.config.lock().unwrap();
    let dir = game_dir(&config).ok_or("Game client not installed")?;
    for p in realmlist_paths(&dir) {
        if p.exists() { return std::fs::read_to_string(&p).map_err(|e| e.to_string()); }
    }
    Err("realmlist.wtf not found".into())
}

#[tauri::command]
fn set_realmlist(state: State<AppState>, content: String) -> Result<(), String> {
    let config = state.config.lock().unwrap();
    let dir = game_dir(&config).ok_or("Game client not installed")?;
    for p in realmlist_paths(&dir) {
        if p.exists() { let _ = std::fs::write(&p, &content); return Ok(()); }
    }
    let fallback = dir.join("realmlist.wtf");
    std::fs::write(&fallback, &content).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn pause_download(state: State<AppState>) {
    state.download_paused.store(true, Ordering::Relaxed);
}

#[tauri::command]
fn resume_download(state: State<AppState>) {
    state.download_paused.store(false, Ordering::Relaxed);
}

#[tauri::command]
async fn repair_game(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    download_client(app, state).await
}

// ── App entry ───────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let default_config: Config = serde_json::from_str(DEFAULT_CONFIG)
        .expect("Failed to parse embedded config.json");

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            let path = config_dir(app.handle());
            let config_path = path.join("config.json");
            let config = load_config(&config_path, &default_config);
            if !config_path.exists() {
                save_config(&config_path, &config);
            }
            app.manage(AppState {
                config: Mutex::new(config),
                config_path,
                download_paused: AtomicBool::new(false),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_settings,
            get_client_status,
            launch_game,
            clear_cache,
            list_installed_addons,
            fetch_addon_list,
            install_addon,
            delete_addon,
            select_game_path,
            download_client,
            get_realmlist,
            set_realmlist,
            open_url,
            pause_download,
            resume_download,
            repair_game,
        ])
        .run(tauri::generate_context!())
        .expect("error while running application");
}
