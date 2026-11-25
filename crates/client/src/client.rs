//! Client lifetime: setup, the main loop, and a clean shutdown.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc, Mutex};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HMODULE, WAIT_OBJECT_0};
use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcessId, Sleep, WaitForSingleObject,
};

use crate::config::{self, Config};
use crate::console::{self, Reader};
use crate::gui;
use crate::input;
use crate::log;
use crate::modules::{self, Modules, Tick};
use crate::net;

fn dll_dir(hmodule: HMODULE) -> PathBuf {
    let mut buf = vec![0u16; 1024];
    let len = unsafe { GetModuleFileNameW(hmodule, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let path = PathBuf::from(String::from_utf16_lossy(&buf[..len.min(buf.len())]));
    path.parent().map(PathBuf::from).unwrap_or_default()
}

/// Named event the injector's `eject` command signals.
fn create_eject_event() -> HANDLE {
    let name: Vec<u16> = format!("Local\\Mio_Client_eject_{}\0", unsafe {
        GetCurrentProcessId()
    })
    .encode_utf16()
    .collect();
    unsafe { CreateEventW(std::ptr::null(), 1, 0, name.as_ptr()) }
}

fn print_help(config: &Config) {
    log!("コマンド一覧:");
    log!("  list                  機能の一覧と状態");
    log!("  inputdebug            通常入力の診断");
    log!("  invdebug              インベントリ移動の診断");
    log!("  blockdebug            ブロックESPの診断");
    log!("  auradebug / triggerdebug  自動攻撃の動作元・対象・回数");
    log!("  fishdebug             自動釣りのイベント・要求（ファイルにも保存）");
    log!("  chatrepeatdebug       繰り返し送信の待機理由・実行回数（ファイルにも保存）");
    log!("  actordebug            エンティティの体力・自分の空腹と経験値");
    log!("  statusdebug           効果・酸素量の診断");
    log!("  worlddebug            時刻・天気・明るさ・FreeLookの診断");
    log!("  freecamdebug          自由視点のカメラ・復帰・入力停止の診断");
    log!("  mapdebug              WorldMapのワールド識別・自動保存状態");
    log!("  skins list|save self|save <16進ID>  スキン画像の一覧・PNG保存（SkinStealerをON）");
    log!("  skins raw self|raw <16進ID>  元テクスチャだけを保存（スキン用PNGとは別）");
    log!("  skindebug             スキン取得・保存状態の診断");
    log!("  compatdebug           更新後のシグネチャ・取得位置・互換性の診断（ファイルにも保存）");
    log!("  profilerdebug         PerformanceProfilerの直近の計測結果（ファイルにも保存）");
    log!("  itemdebug             所持品・防具・自動操作の診断");
    log!("  guarddebug            耐久保護の判断・所持品（ファイルにも保存）");
    log!("  viewdebug             ズーム・明るさ補正の診断");
    log!("  sdk [chat]            SDKのプレイヤー状態・取得したチャット");
    log!("  sdk watch <種類|all> / sdk inspect / sdk unwatch  SDKの読み取りAPIを確認");
    log!("  toggle <機能名>       機能のON/OFF（従来の英語名を指定）");
    log!("  set <機能名> <値>     autoclicker=毎秒回数、timer=倍率、fly=ブロック/秒、");
    log!("                        zoom=倍率、fullbright=ガンマ、killaura/radar=範囲、");
    log!("                        fov=角度、waila=距離、breadcrumbs=点の数、");
    log!("                        crosshair=ピクセル、lowhealth=体力%、playeralert=範囲、");
    log!("                        durabilityguard=切り替える残り耐久度");
    log!("  ip [all]              接続先サーバー・直近の送信先アドレス");
    log!("  wp list|add|del|clear 地点の管理: wp add <名前> [<x> <y> <z>], wp del <名前>");
    log!("  blocks list|add|del|clear  ブロックESPの追加対象（例: mob_spawner）");
    log!("  friends list|add|del|clear  自動攻撃から除外する表示名（空白を含む名前も可）");
    log!("  bind <キー> <コマンド>  キーに割り当て。bind listで一覧、unbindで解除");
    log!("  coords [copy]         現在地の表示・クリップボードへのコピー");
    log!("  profile list|save <名前>|load <名前>  設定の保存・読み込み");
    log!(
        "  eject                 クライアントの解除（{}キー）",
        config.general.eject_key
    );
}

/// Returns true if the client should eject.
fn handle_command(
    line: &str,
    modules: &mut Modules,
    config: &Config,
    profiles: &mut crate::profiles::Profiles,
    binds: &mut crate::binds::Binds,
) -> bool {
    let mut parts = line.split_whitespace();
    match (parts.next(), parts.next(), parts.next()) {
        (Some("help" | "?"), ..) => print_help(config),
        (Some("list"), ..) => modules.print_list(),
        (Some("profilerdebug"), ..) => {
            let text = modules::performance_profiler::diagnostic();
            log!("{text}");
            if let Err(e) = std::fs::write(config.dir.join("Mio_Client_profiler.txt"), text) {
                log!("profilerdebug: {e}");
            }
        },
        (Some("inputdebug"), ..) => log!("{}", crate::game::actions::diagnostic()),
        (Some("compatdebug"), ..) => {
            let text = crate::game::compatibility_diagnostic();
            log!("{text}");
            if let Err(e) = std::fs::write(config.dir.join("Mio_Client_compatdebug.txt"), text) {
                log!("compatdebug: {e}");
            }
        },
        (Some("skins"), ..) => modules::skin_stealer::command(&line.split_whitespace().skip(1).collect::<Vec<_>>(), &config.dir),
        (Some("skindebug"), ..) => {
            let text = crate::game::skins::diagnostic();
            log!("{text}");
            if let Err(e) = std::fs::write(config.dir.join("Mio_Client_skindebug.txt"), text) { log!("skindebug: {e}"); }
        },
        (Some("chatrepeatdebug"), ..) => {
            let status = (0..modules.len())
                .find(|&i| modules.menu_entry(i).0 == "chatrepeat")
                .map(|i| modules.view(i).status)
                .unwrap_or_default();
            let text = format!(
                "ChatRepeat: {status}\n{}\n{}\n{}",
                crate::native_chat::repeat::diagnostic(),
                crate::game::actions::diagnostic(),
                crate::game::inventory::diagnostic()
            );
            log!("{text}");
            if let Err(e) = std::fs::write(config.dir.join("Mio_Client_chatrepeatdebug.txt"), text) {
                log!("chatrepeatdebug: {e}");
            }
        },
        (Some("invdebug"), ..) => log!("{}", crate::game::actions::inventory_diagnostic()),
        (Some("blockdebug"), ..) => log!("{}", crate::game::blocks::diagnostic()),
        (Some("auradebug" | "triggerdebug"), ..) => log!("{}", crate::game::combat::diagnostic()),
        (Some("actordebug"), ..) => log!("{}", crate::game::actors::diagnostic()),
        (Some("statusdebug"), ..) => log!("{}",crate::game::status::diagnostic()),
        (Some("itemdebug"), ..) => log!("{}\n{}",crate::game::items::diagnostic(),crate::game::durability_guard::diagnostic()),
        (Some("guarddebug"), ..) => {
            let enabled=(0..modules.len()).find_map(|i|{let (name,on)=modules.menu_entry(i);(name=="durabilityguard").then_some(on)}).unwrap_or(false);
            let text=format!("durabilityguard module enabled={enabled}\n{}\n{}\n{}\n{}",crate::game::durability_guard::diagnostic(),crate::game::items::diagnostic(),crate::game::actions::diagnostic(),crate::game::inventory::diagnostic());
            log!("{text}");
            if let Err(e)=std::fs::write(config.dir.join("Mio_Client_guarddebug.txt"),text) {log!("guarddebug: {e}");}
        },
        (Some("fishdebug"), ..) => {
            let text=crate::game::fishing::diagnostic();
            log!("{text}");
            if let Err(e)=std::fs::write(config.dir.join("Mio_Client_fishdebug.txt"),text) {log!("fishdebug: {e}");}
        },
        (Some("mapdebug"), ..) => {
            let text = modules::world_map::storage_diagnostic();
            log!("WorldMap: {text}");
            if let Err(e) = std::fs::write(config.dir.join("Mio_Client_mapdebug.txt"), text) {
                log!("mapdebug: {e}");
            }
        },
        (Some("worlddebug"), ..) => {
            let text=format!("{}\n{}\n{}",crate::game::world_info::diagnostic(),crate::game::blocks::lighting::diagnostic(),crate::game::freelook::diagnostic());
            log!("{text}");if let Err(e)=std::fs::write(config.dir.join("Mio_Client_worlddebug.txt"),text){log!("worlddebug: {e}");}
        },
        (Some("freecamdebug"), ..) => {
            let text = format!("{}\n{}\n{}", crate::game::freecam::diagnostic(),
                crate::game::freelook::diagnostic(), crate::game::actions::diagnostic());
            log!("{text}");
            if let Err(e) = std::fs::write(config.dir.join("Mio_Client_freecamdebug.txt"), text) {
                log!("freecamdebug: {e}");
            }
        },
        (Some("gamedebug"), ..) => match crate::game::debug_pointers() {
            // Also written next to the DLL so an external read-only tool can pick it up.
            Some((image, client, player)) => {
                let text = format!("image={image:#x} client={client:#x} player={player:#x}");
                let _ = std::fs::write(config.dir.join("Mio_Client_ptrs.txt"), &text);
                log!("{text}");
            }
            None => log!("gamedebug: ワールドの情報がありません。メニューを閉じて再度お試しください"),
        },
        (Some("viewdebug"), ..) => {
            let (calls, fov, zoom) = crate::game::visual::zoom_stats();
            log!("getFov: 呼び出し{calls}回、直近の視野角{fov:.2}度、ズーム{zoom:.2}倍");
            let (calls, gamma) = crate::game::visual::gamma_stats();
            log!("getGamma: 呼び出し{calls}回、ゲーム本来の値{gamma:.3}");
            let text = crate::game::visual::diagnostic();
            log!("{text}");
            if let Err(error) = std::fs::write(config.dir.join("Mio_Client_viewdebug.txt"), text) {
                log!("viewdebug: {error}");
            }
        }
        (Some("sdk"), Some(action @ ("watch" | "inspect" | "unwatch")), topic) => {
            let text = crate::api::observation_command(action, topic);
            log!("{text}");
            if action == "inspect" {
                if let Err(error) = std::fs::write(config.dir.join("Mio_Client_sdkdebug.txt"), text) {
                    log!("SDK診断の保存: {error}");
                }
            }
        },
        (Some("sdk"), option, _) => {
            match crate::api::game() {
                Ok(game) if option == Some("chat") => match game.chat_since(0) {
                    Ok(batch) => {
                        log!("SDKチャット: {}件、カーソル{}", batch.messages.len(), batch.cursor);
                        for message in batch.messages.iter().rev().take(20).rev() {
                            log!("  {:?}: {}", message.direction, message.text);
                        }
                    },
                    Err(error) => log!("SDK chat: {error}"),
                },
                Ok(game) => match game.player() {
                    Ok(p) => log!("SDK 座標 {:?}; 速度 {:?}; 水平速度 {:.2}ブロック/秒; 水平角 {:.1}, 垂直角 {:.1}; 向き {:?}",
                        p.position, p.velocity, p.horizontal_speed(), p.rotation.yaw, p.rotation.pitch, p.direction()),
                    Err(error) => log!("SDK状態: {error}"),
                },
                Err(error) => log!("SDK: {error}"),
            }
        },
        (Some("toggle"), Some(name), _) => modules.toggle(name),
        (Some("set"), Some(name), Some(value)) => match value.parse::<f64>() {
            Ok(value) => modules.set_param(name, value),
            Err(_) => log!("`{value}`は数値ではありません"),
        },
        (Some("ip" | "serverip"), Some("all"), _) => {
            let rows = net::recent();
            if rows.is_empty() {
                log!("ip: 直近数秒にUDP送信がありません");
            }
            for (address, rate) in rows {
                log!("  {address:<40} {rate}パケット/秒");
            }
        }
        (Some("ip" | "serverip"), ..) => match net::current() {
            Some(address) => log!("接続先: {address}"),
            None => log!("ip: 接続先を検出できません（シングルプレイ・未接続など）。ip allで一覧を表示できます"),
        },
        (Some("wp" | "waypoint"), ..) => {
            let args: Vec<&str> = line.split_whitespace().skip(1).collect();
            modules::waypoints::command(&args);
        }
        (Some("blocks"), ..) => {
            let args: Vec<&str> = line.split_whitespace().skip(1).collect();
            modules::block_esp::command(&args);
        }
        (Some("friends"), ..) => {
            let args:Vec<&str>=line.split_whitespace().skip(1).collect();
            modules::friends::command(&args,modules);
        }
        (Some("bind" | "unbind" | "binds"), ..) => binds.command(line),
        (Some("coords"), option, _) => match crate::game::position() {
            Some((x, y, z)) => {
                let text = format!("{x} {y} {z}");
                let place = crate::game::world::dimension()
                    .map(|(name, _)| format!(" ({name})"))
                    .unwrap_or_default();
                if option == Some("copy") {
                    use hudhook::imgui::ClipboardBackend;
                    crate::clipboard::Clipboard.set(&text);
                    log!("`{text}`をクリップボードへコピーしました");
                    crate::notify::push(&format!("{text}をコピーしました"));
                } else {
                    log!("{text}{place}");
                }
            }
            None => log!("coords: ワールド未接続です"),
        },
        (Some("profile"), ..) => {
            let rest = line.trim_start().strip_prefix("profile").unwrap_or("").trim_start();
            // A file path keeps its own spacing: pass the rest of the line whole.
            if let Some(path) = rest.strip_prefix("import").filter(|p| p.is_empty() || p.starts_with(char::is_whitespace)) {
                profiles.command(&["import", path.trim()], modules);
            } else {
                let args: Vec<&str> = rest.split_whitespace().collect();
                profiles.command(&args, modules);
            }
        }
        (Some("eject" | "exit" | "quit"), ..) => return true,
        _ => log!("不明なコマンド: `{line}`（helpで一覧を表示）"),
    }
    false
}

/// Runs the client until ejected. Returns true if the caller should unload
/// the DLL itself (false: it stays loaded, or hudhook unloads it).
pub fn run(hmodule: HMODULE) -> bool {
    log!(
        "{} {}を読み込みました",
        crate::brand::CLIENT_NAME,
        env!("CARGO_PKG_VERSION")
    );

    let config = config::load(&dll_dir(hmodule));
    let eject_key = modules::key_or_warn("general.eject_key", &config.general.eject_key);
    let eject_event = create_eject_event();

    let (tx, rx) = mpsc::channel();
    console::set_command_sink(tx.clone());

    gui::prefetch(&config.gui);
    // Resolve optional native-HUD patches before Runtime installs its hook at
    // the accessor prologue used by those patches' signatures.
    let modules = Arc::new(Mutex::new(Modules::new(&config)));
    let profiles = Arc::new(Mutex::new(crate::profiles::Profiles::new(&config.dir)));
    let mut binds = crate::binds::Binds::load(&config.dir);
    let lock = || modules.lock().unwrap_or_else(|e| e.into_inner());
    let mut runtime = match unsafe { crate::api::Runtime::attach(&config.offsets) } {
        Ok(runtime) => Some(runtime),
        Err(error) => {
            log!("SDKを利用できません: {error}");
            None
        }
    };

    // Keep the game hook on for the whole session: whether it is being
    // sampled is how we know the player is in the world view.
    let hook_held = runtime.is_some() || crate::game::acquire().is_ok();
    // Watch outbound packets from the start, so the server is already known
    // by the time `ip` is asked for.
    net::init();
    net::refresh();
    print_help(&config);
    lock().print_list();

    gui::window_layout::load(&config.dir);
    let overlay = gui::start(
        hmodule,
        &config.gui,
        eject_key,
        Arc::clone(&modules),
        Arc::clone(&profiles),
    );
    // The in-game console lives in the overlay; without it, or on request,
    // open a console window (it replays what was logged so far).
    let window = config.gui.console_window || !overlay;
    let owns_console = window && console::attach();
    let reader = if window { Reader::spawn(tx) } else { None };
    // From here on, switching a feature shows a notification.
    crate::notify::ready();
    modules::world_map::start_storage(&config.dir, &config.worldmap);

    loop {
        let signaled = !eject_event.is_null()
            && unsafe { WaitForSingleObject(eject_event, 0) } == WAIT_OBJECT_0;
        let focused = input::is_foreground();
        modules::settings::key_capture::poll(focused);
        config::menu_key::poll(&config.dir);
        gui::window_layout::poll(&config.dir, false);
        net::refresh();
        modules::debug_hud::poll_hardware();
        modules::skin_stealer::poll_exports();
        modules::world_map::poll_storage();
        if signaled
            || gui::EJECT_REQUESTED.load(Ordering::Relaxed)
            || (focused && !modules::settings::key_capture::blocked() && input::key_down(eject_key))
        {
            break;
        }

        // The game does not reliably hide the system cursor while playing, so
        // prefer the hook: it is sampled only while the HUD is drawn, i.e. in
        // the world view with no screen open.
        let in_world = if hook_held {
            crate::game::position().is_some()
        } else {
            input::cursor_hidden()
        };
        let playing = focused && !gui::captures_input();
        let paused = if !focused {
            Some("ゲームが非アクティブ")
        } else if !playing {
            Some("メニュー・地図を表示中")
        } else if !in_world && config.general.require_hidden_cursor {
            Some("ワールドの操作画面ではありません")
        } else {
            None
        };

        {
            let mut modules = lock();
            let mut eject = false;
            while let Ok(line) = rx.try_recv() {
                eject |= handle_command(
                    &line,
                    &mut modules,
                    &config,
                    &mut profiles.lock().unwrap_or_else(|e| e.into_inner()),
                    &mut binds,
                );
            }
            if eject {
                break;
            }

            // Binds fire only in the world view with nothing open, so typing a
            // bound letter into chat or a menu does not run its command.
            let in_view = if hook_held {
                crate::game::in_world_view_now()
            } else {
                input::cursor_hidden()
            };
            for command in binds.poll(input::key_down, playing && in_view) {
                console::submit(&command);
            }

            modules.poll_hotkeys(focused);
            modules.tick(&Tick {
                active: paused.is_none(),
                paused,
                focused,
                now_ms: unsafe { GetTickCount64() },
            });
        }

        unsafe { Sleep(2) };
    }

    // Shutdown: undo everything, then decide whether unloading is safe.
    gui::DORMANT.store(true, Ordering::Relaxed);
    gui::window_layout::poll(&config.dir, true);
    lock().disable_all();
    modules::world_map::shutdown_storage();
    // Static memory outlives the DLL unless freed here.
    crate::game::blocks::surface::forget();
    let game_ok = if let Some(runtime) = runtime.as_mut() {
        runtime.shutdown()
    } else {
        crate::game::actions::shutdown() && (!hook_held || crate::game::release().is_ok())
    };
    let net_ok = net::uninstall();
    let visual_ok = crate::game::visual::shutdown();
    let timer_ok = modules::timer::uninstall_if_safe();
    let reader_ok = reader.is_none_or(Reader::stop);
    if !eject_event.is_null() {
        unsafe { CloseHandle(eject_event) };
    }

    let unload = timer_ok && reader_ok && game_ok && visual_ok && net_ok;
    if unload {
        log!("クライアントを解除しました");
    } else {
        log!("全機能を無効にしました。DLLはゲームを再起動するまで残ります");
        if !timer_ok {
            log!("（時刻の逆行を防ぐため、ゲーム速度の時計フックを保持しています）");
        }
        if !visual_ok {
            log!("（視界のフックが実行中のため、ゲーム終了まで保持します）");
        }
        if !game_ok {
            log!("（通常入力の後処理が残っているため、完了までゲームのフックを保持します）");
        }
        if !net_ok {
            log!("（通信フックが実行中、または解除できないためDLLを保持します）");
        }
    }
    console::detach(owns_console);

    if unload && overlay {
        // hudhook removes its render hooks on the next frame and then frees
        // the DLL itself; unloading here would pull code out from under it.
        hudhook::eject();
        return false;
    }
    unload
}
