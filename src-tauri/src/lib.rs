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
    client_version: Option<String>,
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

#[derive(Debug, Clone, Deserialize)]
struct ClientManifest {
    version: Option<String>,
    base_url: Option<String>,
    files: Vec<ManifestFile>,
}

#[derive(Debug, Clone, Deserialize)]
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
    installed_size: u64,
    client_path: String,
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

fn save_config(path: &PathBuf, config: &Config) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(config)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    std::fs::write(path, content)
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

/// Resolve the game directory, preferring the launcher's own default install
/// location (clients_dir) whenever it holds a WoW executable. The saved
/// game_path is used as an override for custom installs, but the default
/// location must always be recognized — even without a saved path.
fn resolve_game_dir(config: &Config, app: &tauri::AppHandle) -> Option<PathBuf> {
    let default = clients_dir(app);
    if find_wow_exe(&default).is_some() {
        return Some(default);
    }
    // Nearby install: a client folder sitting next to the launcher binary
    // (e.g. "The Slums Launcher" and "client" in the same directory).
    if let Ok(exe_dir) = app.path().executable_dir() {
        for candidate in [
            exe_dir.join("client"),
            exe_dir.join("The Slums Client"),
            exe_dir.join("World of Warcraft 3.3.5a"),
        ] {
            if find_wow_exe(&candidate).is_some() {
                return Some(candidate);
            }
        }
        // Some installs place the game directly in the launcher's folder.
        if find_wow_exe(&exe_dir).is_some() {
            return Some(exe_dir);
        }
    }
    game_dir(config)
}

/// Scan obvious locations for an existing WoW install that the user could
/// "add" — used to offer adding a found client instead of forcing a download.
fn detect_existing_client(app: &tauri::AppHandle) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut push = |d: PathBuf| {
        if !found.iter().any(|x: &PathBuf| x == &d) {
            found.push(d);
        }
    };

    // Default install location.
    let default = clients_dir(app);
    if find_wow_exe(&default).is_some() {
        push(default);
    }

    // Near the launcher binary.
    if let Ok(exe_dir) = app.path().executable_dir() {
        for candidate in [
            exe_dir.join("client"),
            exe_dir.join("The Slums Client"),
            exe_dir.join("World of Warcraft 3.3.5a"),
        ] {
            if find_wow_exe(&candidate).is_some() {
                push(candidate);
            }
        }
        if find_wow_exe(&exe_dir).is_some() {
            push(exe_dir);
        }
    }

    // ~/Games and ~/Documents common locations.
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        for candidate in [
            home.join("Games").join("World of Warcraft 3.3.5a"),
            home.join("Games").join("wow"),
            home.join("Documents").join("World of Warcraft 3.3.5a"),
            home.join("WoW"),
        ] {
            if find_wow_exe(&candidate).is_some() {
                push(candidate);
            }
        }
    }

    found
}

fn find_wow_exe(dir: &PathBuf) -> Option<PathBuf> {
    let wow = dir.join("wow.exe");
    if wow.exists() { return Some(wow); }
    let patcher = dir.join("patchmenu.exe");
    if patcher.exists() { return Some(patcher); }

    let entries = std::fs::read_dir(dir).ok()?;
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if name == "wowclassic.exe" || name == "world of warcraft.exe" {
            return Some(entry.path());
        }
        let path = entry.path();
        if path.extension().map(|e| e == "exe").unwrap_or(false) {
            return Some(path);
        }
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            subdirs.push(path);
        }
    }
    for subdir in subdirs {
        if let Some(found) = find_wow_exe(&subdir) {
            return Some(found);
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
    let mut wrote = false;
    for p in &realmlist_paths(game_dir) {
        if p.exists() {
            let _ = std::fs::write(p, &content);
            wrote = true;
        }
    }
    if !wrote {
        let fallback = game_dir.join("Data").join("enUS");
        let _ = std::fs::create_dir_all(&fallback);
        let _ = std::fs::write(fallback.join("realmlist.wtf"), &content);
    }
}

/// Write a sensible first-run Config.wtf (windowed mode, 1080p) only if the
/// user has not created one yet. Config.wtf is a soft path — once the user
/// edits it, we never touch it again.
fn ensure_default_config(game_dir: &PathBuf) {
    // WoW reads WTF/Config.wtf (created on first run). Don't clobber an
    // existing config — that belongs to the user.
    let path = game_dir.join("WTF").join("Config.wtf");
    if path.exists() { return; }

    // Detect the primary monitor size on Windows via the win32 API; on other
    // platforms (or on failure) fall back to 1920x1080.
    let (w, h) = detect_desktop_resolution();

    let content = format!(
        "SET gxWindow \"1\"\n\
         SET gxMaximize \"1\"\n\
         SET gxResolution \"{}x{}\"\n\
         SET gxRefresh \"60\"\n\
         SET gxMultisample \"1\"\n\
         SET gxEnableAllDisplayModes \"0\"\n\
         SET gamma \"1.0\"\n\
         SET mouseSpeed \"1\"\n\
         SET readTOS \"1\"\n\
         SET readEULA \"1\"\n\
         SET readTerminationWithoutNotice \"1\"\n\
         SET showToolsUI \"0\"\n\
         SET showGameTime \"1\"\n\
         SET showVKeyCodes \"0\"\n\
         SET Sound_EnableMusic \"1\"\n\
         SET Sound_EnableAmbience \"1\"\n\
         SET Sound_EnableAllSound \"1\"\n\
         SET Sound_MusicVolume \"0.4\"\n\
         SET Sound_AmbienceVolume \"0.5\"\n\
         SET Sound_MasterVolume \"1\"\n\
         SET locale \"enUS\"\n\
         SET maximized \"1\"\n\
         SET videoQualityLevel \"3\"\n",
        w, h,
    );

    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_ok() {
            let _ = std::fs::write(&path, content);
        }
    }
}

#[cfg(target_os = "windows")]
fn fix_wow_dpi_scaling(exe: &PathBuf) {
    // WoW 3.3.5a is DPI-unaware; on scaled displays its window drifts/falls
    // when dragged. Set the AppCompat "run as DPI unaware" override via reg.exe.
    // The Layers value format is "<flags>" where DPIUNAWARE is the flag.
    let Some(name) = exe.file_name().and_then(|n| n.to_str()) else { return };
    // HKCU so no admin rights are needed. AppCompatFlags\Layers is read for
    // both HKLM and HKCU; HKCU is the per-user compatibility layer.
    let key = r"HKCU\Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Layers";
    let _ = std::process::Command::new("reg")
        .args(["add", key, "/v", name, "/t", "REG_SZ", "/d", "~ DPIUNAWARE", "/f"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(not(target_os = "windows"))]
fn fix_wow_dpi_scaling(_exe: &PathBuf) {}

#[cfg(target_os = "windows")]
fn detect_desktop_resolution() -> (u32, u32) {
    // Use winapi's GetSystemMetrics(SM_CXSCREEN / SM_CYSCREEN) for the
    // primary display. This is a best-effort call; any failure falls back to
    // 1920x1080.
    let (mut w, mut h) = (0u32, 0u32);
    unsafe {
        let user32 = winapi::um::libloaderapi::GetModuleHandleA(c"user32.dll".as_ptr());
        if !user32.is_null() {
            type GetSystemMetricsFn = unsafe extern "system" fn(i32) -> i32;
            let proc = winapi::um::libloaderapi::GetProcAddress(user32, c"GetSystemMetrics".as_ptr());
            if !proc.is_null() {
                let f: GetSystemMetricsFn = std::mem::transmute(proc);
                w = f(0) as u32; // SM_CXSCREEN
                h = f(1) as u32; // SM_CYSCREEN
            }
        }
    }
    if w == 0 || h == 0 { (1920, 1080) } else { (w, h) }
}

#[cfg(not(target_os = "windows"))]
fn detect_desktop_resolution() -> (u32, u32) {
    (1920, 1080)
}

fn compute_sha256(path: &PathBuf) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|e| e.to_string())?;
    Ok(hex::encode(hasher.finalize()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn clients_dir(app: &tauri::AppHandle) -> PathBuf {
    let exe_dir = app.path()
        .executable_dir()
        .unwrap_or_else(|_| PathBuf::from("."));
    // "Follow the launcher" only works when the launcher's own directory is
    // writable. Installs under Program Files (or other protected dirs) reject
    // writes for normal users, so fall back to a user-writable location.
    let probe = exe_dir.join(".launcher_write_probe");
    if std::fs::write(&probe, b"").is_ok() {
        let _ = std::fs::remove_file(&probe);
        exe_dir.join("client")
    } else {
        app.path()
            .app_data_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("client")
    }
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))
}

async fn download_file(client: &reqwest::Client, url: &str, label: &str) -> Result<Vec<u8>, String> {
    let mut last_err = String::new();
    for attempt in 1..=3 {
        if attempt > 1 {
            tokio::time::sleep(Duration::from_millis(400 * attempt as u64)).await;
        }
        match tokio::time::timeout(Duration::from_secs(600), async {
            let response = client.get(url).send().await
                .map_err(|e| format!("Failed: {} - {}", label, e))?;
            if !response.status().is_success() {
                return Err(format!("HTTP {} for: {}", response.status(), label));
            }
            response.bytes().await.map_err(|e| format!("Download error: {}", e))
        }).await {
            Ok(Ok(bytes)) => return Ok(bytes.to_vec()),
            Ok(Err(e)) => last_err = e,
            Err(_) => last_err = format!("Timed out downloading: {}", label),
        }
    }
    Err(last_err)
}

fn is_soft_path(path: &str) -> bool {
    let p = path.replace('\\', "/").to_lowercase();
    // Addons, fonts, and user-modifiable files must never be overwritten or
    // trigger a re-sync. WTF/ holds per-user UI settings, Config.wtf and
    // realmlist.wtf are user-edited, cache/logs/screenshots churn constantly.
    p.starts_with("interface/addons/")
        || p.starts_with("fonts/")
        || p.starts_with("wtf/")
        || p.starts_with("cache/")
        || p.starts_with("logs/")
        || p.starts_with("screenshots/")
        || p == "config.wtf"
        || p == "realmlist.wtf"
        || p == "errors.txt"
        || p.ends_with(".log")
}

fn client_needs_sync(dir: &PathBuf, manifest: &ClientManifest) -> bool {
    for file in &manifest.files {
        if is_soft_path(&file.path) { continue; }
        let local = dir.join(&file.path);
        if local.is_dir() { continue; }
        if !local.exists() { return true; }
        // Size first (cheap), then hash (expensive but catches corruption).
        if let Some(expected) = file.size {
            if std::fs::metadata(&local).map(|m| m.len()).unwrap_or(0) != expected {
                return true;
            }
        }
        if !file.sha256.is_empty() {
            if compute_sha256(&local).map_or(true, |h| h != file.sha256) {
                return true;
            }
        }
    }
    false
}

fn cleanup_tmp_files(dir: &PathBuf) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Ok(ft) = entry.file_type() {
                if ft.is_dir() {
                    cleanup_tmp_files(&path);
                } else if path.extension().map(|e| e == "tmp").unwrap_or(false) {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }
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
    state.config.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

#[tauri::command]
fn save_settings(state: State<AppState>, game_path: Option<String>) -> Result<(), String> {
    let path = state.config_path.clone();
    let mut config = state.config.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(p) = game_path {
        config.game_path = Some(p);
        save_config(&path, &config).map_err(|e| format!("Failed to save settings: {}", e))?;
    }
    Ok(())
}

#[tauri::command]
async fn get_client_status(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<ClientStatus, String> {
    let config = state.config.lock().unwrap_or_else(|e| e.into_inner()).clone();

    let dir = resolve_game_dir(&config, &app);

    match dir {
        Some(d) if find_wow_exe(&d).is_some() => {
            let path_str = d.to_string_lossy().to_string();

            // Best-effort manifest check: unreachable server must not block play —
            // offline falls back to "ready" (client is usable as-is).
            let manifest = match http_client() {
                Ok(client) => {
                    match tokio::time::timeout(
                        Duration::from_secs(10),
                        client.get(&config.manifest_url).send(),
                    ).await {
                        Ok(Ok(resp)) => resp.json::<ClientManifest>().await.ok(),
                        _ => None,
                    }
                }
                Err(_) => None,
            };

            let scan_dir = d.clone();
            let scan_manifest = manifest.clone();
            let (size, needs_sync) = tokio::task::spawn_blocking(move || {
                let size = installed_size(&scan_dir);
                let bad = scan_manifest.as_ref()
                    .map(|m| client_needs_sync(&scan_dir, m))
                    .unwrap_or(false);
                (size, bad)
            }).await
                .map_err(|e| format!("Scan failed: {}", e))?;

            let version_ok = manifest.as_ref()
                .map_or(true, |m| config.client_version.as_deref() == m.version.as_deref());
            let phase = if !version_ok || needs_sync { "needs_update" } else { "ready" };

            Ok(ClientStatus {
                phase: phase.into(),
                installed_size: size,
                client_path: path_str,
            })
        }
        _ => Ok(ClientStatus {
            phase: "not_installed".into(),
            installed_size: 0,
            client_path: clients_dir(&app).to_string_lossy().to_string(),
        })
    }
}

#[tauri::command]
fn detect_clients(app: tauri::AppHandle) -> Vec<String> {
    detect_existing_client(&app)
        .into_iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect()
}

#[tauri::command]
async fn launch_game(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    // Clone the bits we need so the lock guard is dropped before any await.
    let (dir, server_address) = {
        let config = state.config.lock().unwrap_or_else(|e| e.into_inner());
        let dir = resolve_game_dir(&config, &app)
            .ok_or("Game client not installed. Click Install to download it.")?;
        (dir, config.server_address.clone())
    };
    let exe = find_wow_exe(&dir).ok_or("Could not find WoW executable")?;
    ensure_realmlist(&dir, &server_address);
    ensure_default_config(&dir);

    // Windows: WoW 3.3.5a is DPI-unaware — on scaled displays its window
    // "falls" in the drag direction when moved. Force DPI-unaware via the
    // AppCompat registry override so the OS manages scaling properly.
    #[cfg(target_os = "windows")]
    fix_wow_dpi_scaling(&exe);

    #[cfg(target_os = "linux")]
    {
        let wine_prefix = linux_wine_prefix();
        ensure_dxvk_conf(&wine_prefix);
        // First launch: install DXVK into the isolated prefix via winetricks
        // if it isn't set up yet. This is a one-time cost (downloads DXVK).
        if !dxvk_installed(&wine_prefix) {
            emit(&app, "client-progress", ClientSyncProgress {
                downloaded: 0, total: 1, speed: "".into(),
                phase: "connecting".into(),
                message: "Setting up DXVK (first launch — one-time download)...".into(),
            })?;
            setup_dxvk(&wine_prefix).await?;
        }
    }

    let child = if cfg!(target_os = "linux") {
        let wine_prefix = linux_wine_prefix();
        Command::new("wine")
            .arg(&exe)
            .current_dir(&dir)
            .env("WINEPREFIX", &wine_prefix)
            .env("WINEARCH", "win64")
            .env("DXVK_CONFIG_FILE", wine_prefix.join("dxvk.conf"))
            .env("DXVK_STATE_CACHE_PATH", wine_prefix.join("dxvk-cache"))
            .env("WINE_LARGE_ADDRESS_AWARE", "1")
            .env("WINEDEBUG", "-all")
            .env("mesa_glthread", "true")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    } else {
        Command::new(&exe)
            .current_dir(&dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    };

    match child {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("Failed to launch {}: {}", exe.display(), e)),
    }
}

/// True when DXVK DLLs are present in the prefix's system32.
#[cfg(target_os = "linux")]
fn dxvk_installed(prefix: &PathBuf) -> bool {
    let system32 = prefix.join("drive_c").join("windows").join("system32");
    system32.join("d3d9.dll").exists() || system32.join("d3d11.dll").exists()
}

#[cfg(not(target_os = "linux"))]
fn dxvk_installed(_prefix: &PathBuf) -> bool { true }

/// Run `winetricks -q dxvk` against the isolated prefix. Returns an error
/// with install instructions if winetricks is unavailable.
#[cfg(target_os = "linux")]
async fn setup_dxvk(prefix: &PathBuf) -> Result<(), String> {
    let has_winetricks = std::process::Command::new("sh")
        .args(["-c", "command -v winetricks >/dev/null 2>&1"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !has_winetricks {
        return Err(
            "winetricks not found. Install it first:\n\
             Debian/Ubuntu: sudo apt install winetricks\n\
             Arch: sudo pacman -S --needed winetricks\n\
             Then run 'Check Linux Deps' and try again.".into()
        );
    }

    // Ensure the prefix is bootstrapped before winetricks touches it.
    let _ = std::process::Command::new("wineboot")
        .env("WINEPREFIX", prefix)
        .env("WINEARCH", "win64")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    // Use tokio's async Command so we can await winetricks without blocking.
    let mut child = tokio::process::Command::new("winetricks")
        .env("WINEPREFIX", prefix)
        .env("WINEARCH", "win64")
        .args(["-q", "dxvk"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to start winetricks: {}", e))?;

    // Allow up to 5 minutes for the DXVK download+install.
    match tokio::time::timeout(Duration::from_secs(300), child.wait()).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(_)) => Err("winetricks failed to install DXVK".into()),
        Ok(Err(e)) => Err(format!("winetricks error: {}", e)),
        Err(_) => Err("DXVK setup timed out after 5 minutes".into()),
    }
}

#[cfg(not(target_os = "linux"))]
async fn setup_dxvk(_prefix: &PathBuf) -> Result<(), String> { Ok(()) }

/// Wine prefix used for the WoW client. Kept alongside the client data dir
/// so nothing depends on the user's existing ~/.wine (which may be a
/// different prefix/arch and break the game).
#[cfg(target_os = "linux")]
fn linux_wine_prefix() -> PathBuf {
    let home = std::env::var_os("HOME").unwrap_or_else(|| ".".into());
    PathBuf::from(home).join(".local/share/slums-launcher/wine")
}

#[cfg(not(target_os = "linux"))]
fn linux_wine_prefix() -> PathBuf {
    PathBuf::from(".")
}

/// Write the proven DXVK configuration for WoW 3.3.5a if the prefix lacks
/// one. dxvk.conf lives inside the prefix so it travels with it.
#[cfg(target_os = "linux")]
fn ensure_dxvk_conf(prefix: &PathBuf) {
    let path = prefix.join("dxvk.conf");
    if path.exists() { return; }
    let content = "\
d3d9.maxFrameLatency = 1
d3d9.presentInterval = 1
dxgi.tearFree = True
dxvk.enableStateCache = True
dxvk.enableGraphicsPipelineLibrary = True
d3d9.floatEmulation = strict
";
    if std::fs::create_dir_all(prefix).is_ok() {
        let _ = std::fs::write(&path, content);
    }
}

#[cfg(not(target_os = "linux"))]
fn ensure_dxvk_conf(_prefix: &PathBuf) {}

#[tauri::command]
fn check_linux_deps() -> Result<serde_json::Value, String> {
    #[cfg(target_os = "linux")]
    {
        let mut deps = Vec::new();
        for (cmd, name) in [("wine", "Wine"), ("wine64", "Wine 64-bit"), ("dxvk", "DXVK")] {
            let found = std::process::Command::new("sh")
                .args(["-c", &format!("command -v {} >/dev/null 2>&1", cmd)])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            deps.push(serde_json::json!({"name": name, "command": cmd, "installed": found}));
        }
        // Vulkan loader check (32-bit needs i386 on Debian).
        let vulkan_ok = std::process::Command::new("sh")
            .args(["-c", "command -v vulkaninfo >/dev/null 2>&1 && vulkaninfo --summary >/dev/null 2>&1"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        return Ok(serde_json::json!({
            "deps": deps,
            "vulkan_working": vulkan_ok,
        }));
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(serde_json::json!({"linux": false}))
    }
}

#[tauri::command]
fn clear_cache(state: State<AppState>) -> Result<(), String> {
    let config = state.config.lock().unwrap_or_else(|e| e.into_inner());
    let dir = game_dir(&config).ok_or("Game client not installed")?;

    for d in [dir.join("WDB"), dir.join("Cache")] {
        if d.exists() { std::fs::remove_dir_all(&d).map_err(|e| e.to_string())?; }
    }
    Ok(())
}

#[tauri::command]
fn list_installed_addons(state: State<AppState>) -> Vec<InstalledAddon> {
    let config = state.config.lock().unwrap_or_else(|e| e.into_inner());
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
    let config = state.config.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if config.addons_url.is_empty() { return Ok(vec![]); }

    let client = http_client()?;
    let resp = tokio::time::timeout(Duration::from_secs(30), client.get(&config.addons_url).send())
        .await
        .map_err(|_| "Timed out fetching addon list".to_string())?
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
        let config = state.config.lock().unwrap_or_else(|e| e.into_inner());
        let dir = game_dir(&config).ok_or("Game client not installed")?;
        let a_dir = dir.join("Interface").join("AddOns");
        std::fs::create_dir_all(&a_dir).map_err(|e| e.to_string())?;
        let t_dir = app.path().app_data_dir().unwrap_or_else(|_| PathBuf::from("."));
        std::fs::create_dir_all(&t_dir).map_err(|e| e.to_string())?;
        (a_dir, t_dir)
    };

    let zip_path = temp_dir.join(format!("addon_{}.zip", rand_id()));
    let client = http_client()?;
    let bytes = download_file(&client, &download_url, "addon download").await?;

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
    let config = state.config.lock().unwrap_or_else(|e| e.into_inner());
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
    let config = state.config.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let dir = game_dir(&config).unwrap_or_else(|| clients_dir(&app));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    cleanup_tmp_files(&dir);

    emit(&app, "client-progress", ClientSyncProgress {
        downloaded: 0, total: 0, speed: "".into(),
        phase: "connecting".into(), message: "Fetching manifest...".into(),
    })?;

    let client = http_client()?;
    let resp = tokio::time::timeout(Duration::from_secs(30), client.get(&config.manifest_url).send())
        .await
        .map_err(|_| "Timed out fetching manifest".to_string())?
        .map_err(|e| format!("Failed to fetch manifest: {}", e))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("Server returned {} for manifest", status));
    }
    let manifest: ClientManifest = resp.json().await
        .map_err(|e| format!("Invalid manifest (HTTP {}): {}", status, e))?;

    let total = manifest.files.len() as u64;
    let base_url = manifest.base_url.unwrap_or_default();

    emit(&app, "client-progress", ClientSyncProgress {
        downloaded: 0, total, speed: "".into(),
        phase: "scanning".into(),
        message: format!("Scanning {} files...", total),
    })?;

    let mut to_download: Vec<&ManifestFile> = Vec::new();
    let mut needs_hash: Vec<(usize, PathBuf)> = Vec::new();

    for (i, file) in manifest.files.iter().enumerate() {
        let local = dir.join(&file.path);
        if local.is_dir() { continue; }
        if !local.exists() {
            to_download.push(file);
            continue;
        }
        if is_soft_path(&file.path) {
            // Addon/font files: user may have installed or updated them — never overwrite.
            continue;
        }
        if let Some(expected) = file.size {
            if std::fs::metadata(&local).map(|m| m.len()).unwrap_or(0) != expected {
                to_download.push(file);
                continue;
            }
        }
        needs_hash.push((i, local));
    }

    let mut hash_handles = Vec::new();
    for (i, local) in &needs_hash {
        let idx = *i;
        let sha = manifest.files[idx].sha256.clone();
        let local = local.clone();
        hash_handles.push(tokio::task::spawn_blocking(move || {
            (idx, compute_sha256(&local).ok().map_or(false, |h| h == sha))
        }));
    }
    for handle in hash_handles {
        let (idx, match_) = handle.await.map_err(|e| format!("Hash task failed: {}", e))?;
        if !match_ {
            to_download.push(&manifest.files[idx]);
        }
    }

    emit(&app, "client-progress", ClientSyncProgress {
        downloaded: total - 1, total, speed: "".into(),
        phase: "scanning".into(),
        message: format!("Scanned {} files ({} to sync)", total, to_download.len()),
    }).ok();

    if to_download.is_empty() {
        let exe = find_wow_exe(&dir).ok_or("Client files exist but no WoW.exe found")?;
        let exe_str = exe.to_string_lossy().to_string();
        ensure_realmlist(&dir, &config.server_address);
        {
            let mut cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
            cfg.game_path = Some(exe_str.clone());
            cfg.client_version = manifest.version.clone();
            let path = state.config_path.clone();
            save_config(&path, &cfg).map_err(|e| format!("Failed to save game path: {}", e))?;
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

        let bytes = download_file(&client, &file_url, &file.path).await?;

        // Verify downloaded file against manifest before accepting it.
        // Truncated/bad transfers are re-requested (up to 3 attempts inside
        // download_file), so a mismatch here means the file is genuinely broken.
        if let Some(expected_size) = file.size {
            if bytes.len() as u64 != expected_size {
                return Err(format!(
                    "Size mismatch for {}: got {} bytes, expected {}",
                    file.path, bytes.len(), expected_size
                ));
            }
        }
        if !file.sha256.is_empty() {
            let actual_sha = sha256_hex(&bytes);
            if actual_sha != file.sha256 {
                return Err(format!(
                    "Hash mismatch for {}: got {}, expected {}",
                    file.path, actual_sha, file.sha256
                ));
            }
        }
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
        let mut cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
        cfg.game_path = Some(exe_str.clone());
        let path = state.config_path.clone();
        save_config(&path, &cfg).map_err(|e| format!("Failed to save game path: {}", e))?;
    }

    emit(&app, "client-progress", ClientSyncProgress {
        downloaded: total, total, speed: "".into(),
        phase: "complete".into(), message: "Game ready!".into(),
    }).ok();

    Ok(exe_str)
}

#[tauri::command]
fn get_realmlist(state: State<AppState>) -> Result<String, String> {
    let config = state.config.lock().unwrap_or_else(|e| e.into_inner());
    let dir = game_dir(&config).ok_or("Game client not installed")?;
    for p in realmlist_paths(&dir) {
        if p.exists() { return std::fs::read_to_string(&p).map_err(|e| e.to_string()); }
    }
    Err("realmlist.wtf not found".into())
}

#[tauri::command]
fn set_realmlist(state: State<AppState>, content: String) -> Result<(), String> {
    let config = state.config.lock().unwrap_or_else(|e| e.into_inner());
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

// ── Client Tools ─────────────────────────────────────────────

const OPTIMIZER_RELEASE_URL: &str = "https://api.github.com/repos/suprepupre/wow-optimize/releases/latest";
const OPTIMIZER_ASSET_PATTERN: &str = "Release.7z";

#[tauri::command]
fn launch_patcher(state: State<AppState>) -> Result<(), String> {
    let config = state.config.lock().unwrap_or_else(|e| e.into_inner());
    let dir = game_dir(&config).ok_or("Game client not installed")?;

    // Prefer the root patchmenu.exe (the HD client's toggle), fall back to the
    // nested patchmenu/ folder copy.
    let candidates = [
        dir.join("patchmenu.exe"),
        dir.join("patchmenu").join("patchmenu.exe"),
    ];
    let exe = candidates.iter().find(|p| p.exists())
        .ok_or("patchmenu.exe not found in client — run Update Game first")?;

    let child = std::process::Command::new(exe)
        .current_dir(&dir)
        .spawn()
        .map_err(|e| format!("Failed to launch patcher: {}", e))?;
    let _ = child;
    Ok(())
}

#[tauri::command]
fn is_optimizer_installed(state: State<AppState>) -> bool {
    let config = state.config.lock().unwrap_or_else(|e| e.into_inner());
    let Some(dir) = game_dir(&config) else { return false };
    dir.join("wow_optimize.dll").exists() || dir.join("version.dll").exists()
}

#[tauri::command]
async fn install_optimizer(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let config = state.config.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let dir = game_dir(&config).ok_or("Game client not installed")?;

    emit(&app, "client-progress", ClientSyncProgress {
        downloaded: 0, total: 1, speed: "".into(),
        phase: "connecting".into(), message: "Fetching latest wow-optimize release...".into(),
    })?;

    // Find the latest release asset URL.
    let client = http_client()?;
    let resp = tokio::time::timeout(Duration::from_secs(30), client.get(OPTIMIZER_RELEASE_URL)
        .header("User-Agent", "the-slums-launcher").send())
        .await.map_err(|_| "Timed out fetching release info".to_string())?
        .map_err(|e| format!("Failed to fetch release info: {}", e))?;
    let release: serde_json::Value = resp.json().await
        .map_err(|e| format!("Invalid release JSON: {}", e))?;
    let asset_url = release["assets"].as_array().and_then(|arr| arr.iter().find(|a|
        a["name"].as_str().map_or(false, |n| n.contains(OPTIMIZER_ASSET_PATTERN))))
        .and_then(|a| a["browser_download_url"].as_str().map(String::from))
        .ok_or("Could not find optimizer release asset (Release.7z)".to_string())?;

    emit(&app, "client-progress", ClientSyncProgress {
        downloaded: 0, total: 1, speed: "".into(),
        phase: "syncing".into(), message: "Downloading wow-optimize...".into(),
    })?;

    // Download the archive.
    let bytes = download_file(&client, &asset_url, "wow-optimize").await?;

    // Write to a temp archive, extract, remove.
    let tmp_archive = dir.join(".wow-optimize.7z");
    std::fs::write(&tmp_archive, &bytes).map_err(|e| e.to_string())?;

    let result = extract_archive(&tmp_archive, &dir);
    let _ = std::fs::remove_file(&tmp_archive);
    result?;

    emit(&app, "client-progress", ClientSyncProgress {
        downloaded: 1, total: 1, speed: "".into(),
        phase: "complete".into(), message: "wow-optimize installed!".into(),
    })?;
    Ok("Installed".into())
}

#[tauri::command]
fn remove_optimizer(state: State<AppState>) -> Result<(), String> {
    let config = state.config.lock().unwrap_or_else(|e| e.into_inner());
    let dir = game_dir(&config).ok_or("Game client not installed")?;
    for name in ["wow_optimize.dll", "version.dll", "wow_optimize_launcher.exe"] {
        let p = dir.join(name);
        if p.exists() { std::fs::remove_file(&p).map_err(|e| format!("Failed to remove {}: {}", name, e))?; }
    }
    Ok(())
}

#[tauri::command]
fn launch_optimizer(state: State<AppState>) -> Result<(), String> {
    let config = state.config.lock().unwrap_or_else(|e| e.into_inner());
    let dir = game_dir(&config).ok_or("Game client not installed")?;
    let exe = dir.join("wow_optimize_launcher.exe");
    if !exe.exists() {
        return Err("wow-optimize is not installed — click Install first".into());
    }
    let child = std::process::Command::new(&exe)
        .current_dir(&dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to launch optimizer: {}", e))?;
    let _ = child;
    Ok(())
}

fn extract_archive(archive: &PathBuf, dest: &PathBuf) -> Result<(), String> {
    // 7z archive: use the 7z binary to extract flat (no subfolder). The release
    // is a flat archive containing version.dll, wow_optimize.dll and
    // wow_optimize_launcher.exe directly at the client root.
    for name in ["version.dll", "wow_optimize.dll", "wow_optimize_launcher.exe"] {
        let target = dest.join(name);
        if target.exists() { let _ = std::fs::remove_file(&target); }
    }
    let archive_str = archive.to_string_lossy().to_string();
    let dest_str = dest.to_string_lossy().to_string();
    // `7z e` extracts all files flat into the destination (no paths).
    for cmd in ["7z", "7za", "7zz", "bsdtar"] {
        let args: Vec<String> = match cmd {
            "bsdtar" => vec!["-xf".into(), archive_str.clone(), "-C".into(), dest_str.clone()],
            _ => vec!["e".into(), archive_str.clone(), format!("-o{}", dest_str)],
        };
        if let Ok(out) = std::process::Command::new(cmd).args(&args).output() {
            if out.status.success() { return Ok(()); }
        }
    }
    Err("Could not extract archive — 7z not available".into())
}

// ── Updates (GitHub API — repo is public, no signing needed) ──

const RELEASE_API: &str = "https://api.github.com/repos/joshhmann/the-slums-launcher/releases/latest";

fn is_newer(remote: &str, current: &str) -> bool {
    fn parse(v: &str) -> Vec<u64> {
        v.trim_start_matches('v')
            .split(['.', '-'])
            .filter_map(|p| p.parse::<u64>().ok())
            .collect()
    }
    let (r, c) = (parse(remote), parse(current));
    for (a, b) in r.iter().zip(c.iter()) {
        if a != b { return a > b; }
    }
    r.len() > c.len()
}

async fn fetch_latest_release() -> Result<serde_json::Value, String> {
    let client = http_client()?;
    let resp = tokio::time::timeout(
        Duration::from_secs(30),
        client.get(RELEASE_API).header("User-Agent", "the-slums-launcher").send(),
    ).await
        .map_err(|_| "Timed out fetching latest release".to_string())?
        .map_err(|e| format!("Failed to fetch release: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("GitHub returned {} for latest release", resp.status()));
    }
    resp.json().await.map_err(|e| format!("Invalid release JSON: {}", e))
}

#[tauri::command]
async fn check_for_update(
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    let release = fetch_latest_release().await?;
    let remote_version = release["tag_name"].as_str().unwrap_or("").trim_start_matches('v').to_string();
    let current_version = app.package_info().version.to_string();

    if remote_version.is_empty() || !is_newer(&remote_version, &current_version) {
        return Ok(serde_json::json!({
            "available": false,
            "current_version": current_version,
            "latest_version": remote_version,
        }));
    }

    Ok(serde_json::json!({
        "available": true,
        "version": remote_version,
        "current_version": current_version,
        "notes": release["body"].as_str().unwrap_or(""),
        "url": release["html_url"].as_str().unwrap_or(""),
    }))
}

#[tauri::command]
async fn download_and_install_update(
    app: tauri::AppHandle,
) -> Result<String, String> {
    let release = fetch_latest_release().await?;
    let remote_version = release["tag_name"].as_str().unwrap_or("").trim_start_matches('v').to_string();
    let current_version = app.package_info().version.to_string();
    if !is_newer(&remote_version, &current_version) {
        return Err("No update available".into());
    }

    // Pick the platform asset.
    #[cfg(target_os = "windows")]
    let (asset_match, install_suffix) = (vec![".msi", ".exe"], ".exe");
    #[cfg(target_os = "linux")]
    let (asset_match, install_suffix) = (vec![".AppImage"], ".AppImage");
    #[cfg(target_os = "macos")]
    let (asset_match, install_suffix) = (vec![".dmg"], ".dmg");

    let mut chosen: Option<(String, String)> = None;
    for a in release["assets"].as_array().unwrap_or(&vec![]) {
        let name = a["name"].as_str().unwrap_or("");
        let has_match = asset_match.iter().any(|m| name.ends_with(m));
        // Prefer NSIS setup exe over raw exe for in-place upgrade on Windows.
        let prefer = if cfg!(target_os = "windows") {
            name.contains("setup") || name.contains("_x64") && !name.ends_with(".msi")
        } else { true };
        if has_match && prefer {
            chosen = Some((name.to_string(), a["browser_download_url"].as_str().unwrap_or("").to_string()));
            break;
        }
    }
    let (asset_name, asset_url) = chosen.ok_or("Could not find a compatible installer for this platform")?;
    let _ = install_suffix;

    emit(&app, "client-progress", ClientSyncProgress {
        downloaded: 0, total: 1, speed: "".into(),
        phase: "connecting".into(),
        message: format!("Downloading {}...", asset_name),
    })?;

    let client = http_client()?;
    let bytes = download_file(&client, &asset_url, &asset_name).await?;

    emit(&app, "client-progress", ClientSyncProgress {
        downloaded: 0, total: 1, speed: "".into(),
        phase: "syncing".into(),
        message: "Installing update...".into(),
    })?;

    // Windows: run the NSIS installer silently (currentUser mode keeps the
    // app data; the installer handles the in-place upgrade).
    // Linux: swap the running AppImage — replace the executable file and
    // relaunch. Tauri AppImages self-replace on next run; simplest reliable
    // path is dropping the new AppImage over the old one's path.
    #[cfg(target_os = "windows")]
    {
        let tmp = std::env::temp_dir().join(&asset_name);
        std::fs::write(&tmp, &bytes).map_err(|e| format!("Failed to write installer: {}", e))?;
        let status = std::process::Command::new(&tmp)
            .arg("/S")
            .spawn()
            .map_err(|e| format!("Failed to start installer: {}", e))?;
        let _ = status;
        return Ok("Installer launched — launcher will restart when complete".into());
    }

    #[cfg(target_os = "linux")]
    {
        // Install new AppImage next to the current one, then relaunch.
        let exe_path = std::env::current_exe().map_err(|e| e.to_string())?;
        let new_path = exe_path.with_extension("new.AppImage");
        std::fs::write(&new_path, &bytes).map_err(|e| format!("Failed to write AppImage: {}", e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&new_path, std::fs::Permissions::from_mode(0o755));
        }
        let _ = std::process::Command::new(&new_path).spawn();
        return Ok("New launcher started".into());
    }

    #[cfg(target_os = "macos")]
    {
        return Err("macOS updates not yet supported".into());
    }
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
                if let Err(e) = save_config(&config_path, &config) {
                    eprintln!("Failed to write config: {}", e);
                }
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
            detect_clients,
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
            launch_patcher,
            is_optimizer_installed,
            install_optimizer,
            remove_optimizer,
            launch_optimizer,
            check_for_update,
            download_and_install_update,
            check_linux_deps,
        ])
        .run(tauri::generate_context!())
        .expect("error while running application");
}


