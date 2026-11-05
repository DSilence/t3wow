use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use t3wow::config::{BRIDGE_VERSION, home_dir, load_config, save_config};
use t3wow::error::{Error, Result};
use t3wow::lock::BridgeLock;
use t3wow::run::{RunOptions, run_bridge};
use t3wow::setup::{SignIn, normalize_mcp_url};
use t3wow::t3::T3Client;
use t3wow::{app, oauth, service, setup, update, url, watch, wow};

fn usage() -> String {
    format!(
        "t3wow {BRIDGE_VERSION} - drive a remote T3 Code environment from World of Warcraft

Usage:
  t3wow setup <mcp-url> [install options]  Sign in, install the addon and the tray app
                                           (or double-click the app and use its Settings)
  t3wow login <mcp-url> [--no-browser]     Sign in to a T3 environment (asks for a pairing code)
  t3wow install [--wow <dir>] [--flavor <name>] [--slots <n>] [--link] [--copy-slots]
                                           Install the addon into World of Warcraft
  t3wow tray [--settings]                  Run the bridge from the menu bar / notification area
  t3wow app install|uninstall|open         The \"T3 WoW\" app that starts the tray (macOS, Windows)
  t3wow run [--wait-for-wow]               Run the bridge in this terminal instead
  t3wow update [--check]                   Install the latest release (the tray app does this itself)
  t3wow status                             Show configuration and sign-in state
  t3wow logout                             Forget the T3 sign-in
  t3wow uninstall                          Remove the addon, its data addons and the app

The MCP URL is the environment's address followed by /mcp. In T3 Code open
Settings > Connections, open the environment's menu and choose \"Copy MCP URL\".
It works with T3 Connect, Tailscale and LAN addresses alike.
"
    )
}

#[derive(Default)]
struct Args {
    positionals: Vec<String>,
    wow: Option<String>,
    flavor: Option<String>,
    slots: Option<String>,
    link: bool,
    copy_slots: bool,
    no_browser: bool,
    wait_for_wow: bool,
    check: bool,
    settings: bool,
    first_run: bool,
    help: bool,
}

fn parse_args() -> Result<Args> {
    let mut args = Args::default();
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name.to_string(), Some(value.to_string())),
            _ => (arg.clone(), None),
        };
        let mut value = |flag: &str| {
            inline.clone().or_else(|| iter.next()).ok_or_else(|| Error::other(format!("{flag} needs a value")))
        };
        match name.as_str() {
            "--wow" => args.wow = Some(value("--wow")?),
            "--flavor" => args.flavor = Some(value("--flavor")?),
            "--slots" => args.slots = Some(value("--slots")?),
            "--link" => args.link = true,
            "--copy-slots" => args.copy_slots = true,
            "--no-browser" => args.no_browser = true,
            "--wait-for-wow" => args.wait_for_wow = true,
            "--check" => args.check = true,
            "--settings" => args.settings = true,
            "--first-run" => args.first_run = true,
            "-h" | "--help" => args.help = true,
            "-V" | "--version" => args.positionals.insert(0, "version".into()),
            flag if flag.starts_with('-') => return Err(Error::other(format!("Unknown option {flag}"))),
            _ => args.positionals.push(arg),
        }
    }
    Ok(args)
}

fn open_in_browser(target: &str) {
    app::open_external(target);
}

fn login(raw_url: Option<&str>, open_browser: bool) -> Result<()> {
    let mut config = load_config()?;
    let target = raw_url
        .map(String::from)
        .or_else(|| config.server_url.clone())
        .ok_or_else(|| Error::other("Pass the environment's MCP URL: t3wow login https://<environment-address>/mcp"))?;
    let server_url = normalize_mcp_url(&target);
    oauth::check_sign_in_supported(&server_url)?;

    let mut client = match T3Client::connect(&server_url) {
        Ok(client) => {
            println!("Already signed in.");
            client
        }
        Err(Error::AuthRequired(_)) => {
            let pending = oauth::begin_login(&server_url, config.oauth_port)?;
            println!(
                "
Approve the bridge on the T3 environment:

  {}

On that page enter a pairing code (Settings > Connections > create pairing link,
or `t3 auth pairing create` on the host). Choose \"Supervised\" or higher to be able
to send messages from the game; \"Read only\" only lets you watch threads.
",
                pending.authorization_url
            );
            if open_browser {
                open_in_browser(&pending.authorization_url);
            }
            println!("Waiting for approval...");
            pending.finish(Duration::from_secs(10 * 60))?;
            T3Client::connect(&server_url)?
        }
        Err(error) => return Err(error),
    };
    let projects = client.list_projects()?;
    println!(
        "Signed in to {}: {} project(s), {}.",
        url::host(&server_url),
        projects.len(),
        if client.can_operate() { "can send messages" } else { "read only" }
    );
    config.server_url = Some(server_url);
    save_config(&config)?;
    Ok(())
}

fn install(args: &Args) -> Result<()> {
    let mut config = load_config()?;
    let roots: Vec<PathBuf> = match &args.wow {
        Some(dir) => vec![std::path::absolute(dir)?],
        None => wow::detect_wow_roots(),
    };
    if roots.is_empty() {
        return Err(Error::other("Could not find World of Warcraft. Pass --wow \"/path/to/World of Warcraft\"."));
    }
    let mut flavors: Vec<PathBuf> = roots.iter().flat_map(|root| setup::flavors_in(root)).collect();
    if let Some(flavor) = &args.flavor {
        let wanted = format!("_{}_", flavor.trim_matches('_'));
        flavors.retain(|dir| dir.file_name().and_then(|n| n.to_str()) == Some(wanted.as_str()));
    }
    if flavors.is_empty() {
        return Err(Error::other("No matching WoW flavor folders (_retail_, _classic_, ...) found."));
    }
    if let Some(slots) = &args.slots {
        let slots = slots.parse::<u32>().map_err(|_| Error::other("--slots needs a number."))?;
        config.live_slots = setup::check_live_slots(slots)?;
    }
    for flavor in &flavors {
        setup::install_into(&mut config, flavor, args.link, args.copy_slots)?;
        println!(
            "Installed into {} ({} live slots{})",
            flavor.display(),
            config.live_slots,
            if args.link { ", linked" } else { "" }
        );
    }
    save_config(&config)?;
    if watch::wow_running() {
        println!("\nWoW is running: restart it, since new addon folders are only picked up at launch.");
    }
    Ok(())
}

fn status() -> Result<()> {
    let config = load_config()?;
    println!("Bridge:        t3wow {BRIDGE_VERSION}, home {}", home_dir().display());
    println!("Environment:   {}", config.server_url.as_deref().unwrap_or("(none, run t3wow login)"));
    if let Some(server_url) = &config.server_url {
        let state = match setup::sign_in_state(server_url) {
            SignIn::No => "no".to_string(),
            SignIn::Expired => "expired, run t3wow login".to_string(),
            SignIn::Yes(Some(days)) => format!("yes, {days} days left"),
            SignIn::Yes(None) => "yes".to_string(),
        };
        println!("Signed in:     {state}");
    }
    if config.wow_flavor_dirs.is_empty() {
        println!("WoW:           (not installed, run t3wow install)");
    }
    for flavor in &config.wow_flavor_dirs {
        let installed = wow::is_installed(flavor);
        let detail = if installed {
            let kind = if wow::slots_linked(flavor) { "linked" } else { "copied" };
            format!("({} live slots, {kind})", wow::installed_live_slots(flavor))
        } else {
            "(missing)".into()
        };
        println!("WoW:           {} {detail}", flavor.display());
        for file in wow::saved_variables_files(flavor) {
            println!("  saved state: {}", file.display());
        }
    }
    println!("Game running:  {}", if watch::wow_running() { "yes" } else { "no" });
    println!(
        "App:           {}",
        if app::installed() {
            app::app_path().display().to_string()
        } else {
            "not installed (run t3wow app install)".into()
        }
    );
    println!("Running:       {}", if BridgeLock::held() { "yes" } else { "no" });
    println!(
        "Updates:       {}",
        if config.auto_update { "installed by the tray app" } else { "off (autoUpdate in config.json)" }
    );
    if service::legacy_installed() {
        println!("Old service:   still installed; `t3wow app install` removes it");
    }
    println!("Log:           {}", service::log_file().display());
    Ok(())
}

static STOP: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
fn handle_signals() {
    extern "C" fn on_signal(_: libc::c_int) {
        STOP.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    unsafe {
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
    }
}

#[cfg(not(unix))]
fn handle_signals() {}

fn run(args: &Args) -> Result<()> {
    let _lock = BridgeLock::acquire()?;
    handle_signals();
    let detector = || watch::wow_running();
    let options = RunOptions {
        load_config: &|| load_config().map_err(Into::into),
        game_running: if args.wait_for_wow { Some(&detector) } else { None },
        stop: &STOP,
        report: None,
    };
    run_bridge(&options)
}

#[cfg(any(target_os = "macos", windows))]
fn tray(args: &Args) -> Result<()> {
    t3wow::tray::run_tray(t3wow::tray::TrayOptions { open_settings: args.settings, first_run: args.first_run })
}

#[cfg(not(any(target_os = "macos", windows)))]
fn tray(_args: &Args) -> Result<()> {
    Err(Error::other("The tray app is for macOS and Windows. On Linux run `t3wow run --wait-for-wow`."))
}

fn remove_old_service() -> Result<()> {
    if service::remove_legacy()? {
        println!("Removed the old login service; the tray app replaces it.");
    }
    Ok(())
}

fn app_command(action: Option<&str>) -> Result<()> {
    match action {
        Some("install") => {
            remove_old_service()?;
            let path = app::install()?;
            println!("Installed {}.", path.display());
            if cfg!(target_os = "macos") {
                println!("Open \"{}\" (Spotlight, Launchpad or ~/Applications) to start the bridge.", app::APP_NAME);
            } else {
                println!("Start \"{}\" from the Start menu to start the bridge.", app::APP_NAME);
            }
            println!("It sits in the menu bar, syncs while World of Warcraft runs, and stops when you quit it.");
            if BridgeLock::held() {
                println!("A bridge is running now: quit it from its menu and open the app again to use this version.");
            }
        }
        Some("uninstall") => {
            let removed = app::uninstall()?;
            println!("{}", if removed { "Removed the app." } else { "The app was not installed." });
        }
        Some("open") => app::open()?,
        // Release builds package the app with this (macOS).
        #[cfg(target_os = "macos")]
        Some("bundle") => {
            let path = args_path(2)?;
            app::write_bundle(&path)?;
            println!("Wrote {}.", path.display());
        }
        _ => return Err(Error::other("Use `t3wow app install`, `t3wow app uninstall` or `t3wow app open`.")),
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn args_path(index: usize) -> Result<PathBuf> {
    let args: Vec<String> = std::env::args().skip(1).filter(|arg| !arg.starts_with("--")).collect();
    let path = args.get(index).ok_or_else(|| Error::other("Pass a path."))?;
    Ok(std::path::absolute(path)?)
}

fn update(args: &Args) -> Result<()> {
    println!("t3wow {BRIDGE_VERSION}: checking for updates...");
    let Some(release) = update::check(&update::latest_release_url(), BRIDGE_VERSION)? else {
        println!("This is the latest version.");
        return Ok(());
    };
    println!("Version {} is available.", release.version);
    if args.check {
        return Ok(());
    }
    if watch::wow_running() {
        return Err(Error::other("Close World of Warcraft first: the update replaces the addon."));
    }
    let file = update::download(&release, &update::download_dir())?;
    print!("{}", update::install(&file)?);
    println!("Updated to {}.", release.version);
    if BridgeLock::held() {
        println!("The running bridge still uses the old version: quit T3 WoW from its menu and open it again.");
    }
    Ok(())
}

fn setup(args: &Args) -> Result<()> {
    let config = load_config()?;
    let url = args.positionals.get(1).map(String::as_str);
    let signed_in = config.server_url.as_deref().is_some_and(|u| oauth::access_token(u).ok().flatten().is_some());
    if url.is_some() || !signed_in {
        login(url, !args.no_browser)?;
    }
    println!();
    install(args)?;
    println!();
    if cfg!(any(target_os = "macos", windows)) {
        app_command(Some("install"))?;
    } else {
        remove_old_service()?;
        println!("Run `t3wow run --wait-for-wow` to start the bridge.");
    }
    println!("\nThen start World of Warcraft and type /t3 in game.");
    Ok(())
}

fn main_inner() -> Result<()> {
    update::clean_up();
    let args = parse_args()?;
    let mut command = if args.help { Some("help") } else { args.positionals.first().map(String::as_str) };
    if command.is_none() && double_clicked() {
        command = Some(if installed_copy() { "tray" } else { "install-app" });
    }
    match command {
        #[cfg(any(target_os = "macos", windows))]
        Some("install-app") => {
            let result = t3wow::installer::install_and_start();
            if let Err(error) = &result {
                t3wow::installer::show_error(&error.to_string());
            }
            result
        }
        Some("setup") => setup(&args),
        Some("login") => login(args.positionals.get(1).map(String::as_str), !args.no_browser),
        Some("install") => install(&args),
        Some("run") => run(&args),
        Some("tray") => tray(&args),
        Some("app") => app_command(args.positionals.get(1).map(String::as_str)),
        Some("service") => match args.positionals.get(1).map(String::as_str) {
            Some("uninstall") => remove_old_service(),
            _ => Err(Error::other("The login service is gone; use the tray app: `t3wow app install`.")),
        },
        Some("status") => status(),
        Some("update") => update(&args),
        // Run by the new binary after `update` swapped it in.
        Some("post-update") => update::post_update(),
        Some("logout") => {
            let config = load_config()?;
            if let Some(server_url) = &config.server_url {
                oauth::forget_auth(server_url)?;
            }
            println!("Signed out. Revoke the session on the host under Settings > Connections if needed.");
            Ok(())
        }
        Some("uninstall") => {
            let mut config = load_config()?;
            service::remove_legacy()?;
            app::uninstall()?;
            for flavor in &config.wow_flavor_dirs {
                wow::uninstall_addon(flavor)?;
            }
            config.wow_flavor_dirs.clear();
            save_config(&config)?;
            println!("Removed the addon and the app.");
            Ok(())
        }
        Some("version") => {
            println!("t3wow {BRIDGE_VERSION}");
            Ok(())
        }
        Some("help") | None => {
            print!("{}", usage());
            Ok(())
        }
        Some(other) => {
            print!("{}", usage());
            Err(Error::other(format!("Unknown command {other}")))
        }
    }
}

/// Opened from Finder or Explorer rather than a terminal: Finder passes no
/// command to "T3 WoW.app", and a double-clicked exe has a console of its own.
fn double_clicked() -> bool {
    #[cfg(windows)]
    {
        t3wow::win::launched_from_explorer()
    }
    #[cfg(not(windows))]
    {
        app::launched_from_bundle()
    }
}

/// The installed app runs the tray; any other copy installs itself first.
#[cfg(any(target_os = "macos", windows))]
fn installed_copy() -> bool {
    app::running_installed_copy()
}

#[cfg(not(any(target_os = "macos", windows)))]
fn installed_copy() -> bool {
    true
}

fn main() -> ExitCode {
    match main_inner() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("t3wow: {error}");
            ExitCode::FAILURE
        }
    }
}
