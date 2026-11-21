//! Regression tests for native input hooks and input ownership.
use super::super::mouse::MOUSE_REBASE;
use super::*;
#[test]
fn a_native_sample_published_after_the_worker_tick_time_is_fresh() {
    assert!(recent_world_input(1000, 1001, 0));
    assert!(recent_world_input(1000, 1000, 0));
    assert!(!recent_world_input(1200, 1000, 0));
    assert!(!recent_world_input(1000, 1001, 2));
    assert!(!recent_world_input(1000, 0, 0));
}

#[test]
fn fishing_only_relaxes_the_own_inventory_screen_gate() {
    for base in 0..=4 {
        assert_eq!(fishing_active(base, false), base == 0);
        assert_eq!(fishing_active(base, true), base == 0 || base == 4);
    }
}
#[test]
fn native_use_exception_is_scoped_to_thread_instance_and_call_lifetime() {
    assert_eq!(use_permission(0, 10), 0);
    with_use_scope(10, || {
        assert_eq!(use_permission(0, 10), 1);
        assert_eq!(use_permission(2, 10), 2);
        assert_eq!(use_permission(0, 20), 0);
        assert_eq!(
            std::thread::spawn(|| use_permission(0, 10)).join().unwrap(),
            0
        );
        with_use_scope(20, || {
            assert_eq!(use_permission(0, 10), 0);
            assert_eq!(use_permission(0, 20), 1);
        });
        assert_eq!(use_permission(0, 10), 1);
    });
    assert_eq!(use_permission(0, 10), 0);
    let _ = std::panic::catch_unwind(|| with_use_scope(10, || panic!("test cleanup")));
    assert_eq!(use_permission(0, 10), 0);
    with_use_scope(0, || assert_eq!(use_permission(0, 0), 0));
}
static INPUT_TEST_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn inventory_permission_override_is_scoped_to_movement_and_local_inventory() {
    assert_eq!(movement_permission(0, true, true), 1);
    assert_eq!(movement_permission(0, true, false), 0);
    assert_eq!(movement_permission(0, false, true), 0);
    assert_eq!(movement_permission(0, false, false), 0);
    assert_eq!(movement_permission(1, false, false), 1);
    assert_eq!(movement_permission(2, true, true), 2);
}

#[test]
fn inventory_exception_never_bypasses_overlay_focus_or_shutdown_gates() {
    for gate in [1, 2, 3] {
        assert_eq!(movement_gate(gate, true), gate);
    }
    assert_eq!(movement_gate(4, false), 4);
    assert_eq!(movement_gate(4, true), 0);
}

#[test]
fn inventory_adopts_pressed_keys_and_returns_held_keys_to_vanilla() {
    let mut held = InputHold {
        manual: true,
        ..InputHold::default()
    };
    held.inventory_ownership(true, true, false);
    assert!(!held.manual && held.owned);
    // A key released while the inventory owns it must be released in the game.
    assert_eq!(
        button_change(false, held.manual, held.owned, true),
        ButtonChange::Release
    );
    // A key still held on inventory close remains a manual input.
    held.inventory_ownership(true, false, true);
    assert!(held.manual && !held.owned);
    assert_eq!(
        button_change(false, held.manual, held.owned, true),
        ButtonChange::None
    );
}

#[test]
fn jump_sources_share_one_input_and_never_jump_in_dry_air() {
    let forward = 1 << 13;
    let all = jump_demand(forward, true, true, true, true, false);
    assert!(all.wants_input(true, false));
    assert!(all.wants_input(false, true));
    assert!(!all.wants_input(false, false));
    // Turning sprint or AutoJump off leaves the remaining grounded request.
    for (sprint, jump) in [(true, false), (false, true)] {
        let demand = jump_demand(forward, sprint, jump, false, true, false);
        assert_eq!(
            button_change(demand.wants_input(true, false), false, true, true),
            ButtonChange::None
        );
    }
    // Leaving the water releases only our input, preserving physical Space.
    let swim = jump_demand(0, false, false, true, true, false);
    assert!(swim.wants_input(false, true));
    assert!(!swim.wants_input(true, false));
    assert_eq!(
        button_change(swim.wants_input(false, false), false, true, true),
        ButtonChange::Release
    );
    assert_eq!(
        button_change(swim.wants_input(false, false), true, true, true),
        ButtonChange::None
    );
}

#[test]
fn jump_helpers_honor_sneak_forward_settings_and_request_expiry() {
    for flags in [1, (1 << 13) | 1] {
        assert!(!jump_demand(flags, true, true, true, false, false).active());
    }
    assert!(!jump_demand(0, false, true, true, true, true).active());
    assert!(jump_demand(0, false, true, false, false, false).wants_input(true, false));
    assert!(jump_demand(0, false, false, true, true, false).wants_input(false, true));
    let enabled = AtomicBool::new(true);
    let until = AtomicU64::new(100);
    assert!(request_active(&enabled, &until, 100));
    assert!(!request_active(&enabled, &until, 101));
    enabled.store(false, Ordering::Release);
    assert!(!request_active(&enabled, &until, 99));
}

#[test]
fn backward_override_releases_automatic_walk_but_preserves_manual_forward() {
    assert!(wants_walk(0, true));
    for backward in [1 << 14, 1 << 11, 1 << 12] {
        let flags = (1 << 13) | backward;
        assert!(!wants_walk(flags, true));
        assert!(wants_walk(flags, false));
        assert_eq!(
            button_change(wants_walk(flags, true), false, true, true),
            ButtonChange::Release
        );
        assert_eq!(
            button_change(wants_walk(flags, true), true, true, true),
            ButtonChange::None
        );
    }
    assert_eq!(
        button_change(wants_walk(0, true), false, false, false),
        ButtonChange::Press
    );
}

#[test]
fn sneak_forward_option_and_sprint_interaction_use_native_movement_flags() {
    assert_eq!(MovementButton::Forward.mask(), 1 << 13);
    assert_eq!(MovementButton::Sneak.mask(), 1);
    assert!(wants_sneak(0, false));
    assert!(!wants_sneak(0, true));
    for forward in [1 << 13, 1 << 9, 1 << 10] {
        assert!(wants_sneak(forward, true));
        assert!(wants_sprint(forward));
        assert!(!wants_sprint(forward | MovementButton::Sneak.mask()));
    }
    assert_eq!(button_change(false, true, true, true), ButtonChange::None);
    assert_eq!(
        button_change(false, false, true, true),
        ButtonChange::Release
    );
}

#[test]
fn opening_menu_releases_owned_walk_and_sneak_without_repressing() {
    let _lock = INPUT_TEST_LOCK.lock().unwrap();
    static UPS: AtomicU64 = AtomicU64::new(0);
    unsafe extern "system" fn movement(instance: usize) -> usize {
        instance
    }
    unsafe extern "system" fn release(closure: *const c_void, _: *const u8, instance: usize) {
        let bit = read::<i32>(closure as usize + 8);
        let flags = (instance + 0x10) as *mut u32;
        flags.write_unaligned(flags.read_unaligned() & !(1 << bit));
        UPS.fetch_add(1, Ordering::Relaxed);
    }
    let old_up = TRACK_UP.swap(release as *const () as usize, Ordering::AcqRel);
    let old_menu = gui::MENU_OPEN.swap(true, Ordering::AcqRel);
    let old_dormant = gui::DORMANT.swap(false, Ordering::AcqRel);
    let mut component = [0u32; 25];
    component[4] = (1 << 13) | 1;
    let instance = component.as_mut_ptr() as usize;
    let owned = || InputHold {
        instance,
        component: instance,
        manual: false,
        owned: true,
    };
    let mut state = State {
        captured: false,
        backward: InputHold::default(),
        left: InputHold::default(),
        right: InputHold::default(),
        forward: owned(),
        sneak: owned(),
        sprint: InputHold::default(),
        jump: InputHold::default(),
        sdk_release: None,
        click: ClickClock { next: 0, epoch: 0 },
    };
    let native = Native { movement };
    unsafe {
        reconcile_movement(&native, instance, &mut state, None);
        reconcile_movement(&native, instance, &mut state, None);
    }
    assert_eq!(component[4], 0);
    assert!(!state.forward.owned && !state.sneak.owned);
    assert_eq!(UPS.load(Ordering::Relaxed), 2);
    TRACK_UP.store(old_up, Ordering::Release);
    gui::MENU_OPEN.store(old_menu, Ordering::Release);
    gui::DORMANT.store(old_dormant, Ordering::Release);
}

#[test]
fn console_clicks_never_reach_attack_and_existing_hold_is_released() {
    let _lock = INPUT_TEST_LOCK.lock().unwrap();
    static DOWNS: AtomicU64 = AtomicU64::new(0);
    static UPS: AtomicU64 = AtomicU64::new(0);
    unsafe extern "system" fn down(_: *const c_void, _: *const u8, _: usize) {
        DOWNS.fetch_add(1, Ordering::Relaxed);
    }
    unsafe extern "system" fn up(_: *const c_void, _: *const u8, _: usize) {
        UPS.fetch_add(1, Ordering::Relaxed);
    }
    let old_down = DOWN.swap(down as *const () as usize, Ordering::AcqRel);
    let old_up = UP.swap(up as *const () as usize, Ordering::AcqRel);
    let invoke_down = || unsafe { down_hook(std::ptr::null(), std::ptr::null(), 1) };
    unsafe extern "system" fn use_allowed(_: usize) -> u8 {
        1
    }
    let old_allowed = USE_ALLOWED.swap(use_allowed as *const () as usize, Ordering::AcqRel);

    gui::DORMANT.store(false, Ordering::Release);
    gui::MENU_OPEN.store(false, Ordering::Release);
    invoke_down();
    assert_eq!(DOWNS.load(Ordering::Relaxed), 1);
    assert!(HELD.load(Ordering::Acquire));
    assert_eq!(unsafe { use_allowed_hook(1) }, 1);

    gui::MENU_OPEN.store(true, Ordering::Release);
    assert_eq!(unsafe { use_allowed_hook(1) }, 0);
    assert_eq!(with_use_scope(1, || unsafe { use_allowed_hook(1) }), 0);
    // Even double-click / repeated console presses must not reach vanilla.
    for _ in 0..3 {
        invoke_down();
    }
    assert_eq!(DOWNS.load(Ordering::Relaxed), 1);
    assert!(HELD.load(Ordering::Acquire));
    unsafe {
        release_attack(1, false, None);
        release_attack(1, false, None);
    }
    assert_eq!(UPS.load(Ordering::Relaxed), 1);
    assert!(!HELD.load(Ordering::Acquire));
    invoke_down();
    assert!(!HELD.load(Ordering::Acquire));
    // Button-up still reaches vanilla while the console captures input.
    unsafe { up_hook(std::ptr::null(), std::ptr::null(), 1) };
    assert_eq!(UPS.load(Ordering::Relaxed), 2);

    gui::MENU_OPEN.store(false, Ordering::Release);
    assert_eq!(unsafe { use_allowed_hook(1) }, 1);
    invoke_down();
    assert_eq!(DOWNS.load(Ordering::Relaxed), 2);
    unsafe { up_hook(std::ptr::null(), std::ptr::null(), 1) };
    // Shutdown must hand normal input back even before the final UI frame.
    gui::MENU_OPEN.store(true, Ordering::Release);
    gui::DORMANT.store(true, Ordering::Release);
    assert_eq!(unsafe { use_allowed_hook(1) }, 1);
    invoke_down();
    assert_eq!(DOWNS.load(Ordering::Relaxed), 3);
    unsafe { up_hook(std::ptr::null(), std::ptr::null(), 1) };

    gui::MENU_OPEN.store(false, Ordering::Release);
    gui::DORMANT.store(false, Ordering::Release);
    DOWN.store(old_down, Ordering::Release);
    UP.store(old_up, Ordering::Release);
    USE_ALLOWED.store(old_allowed, Ordering::Release);
    assert_eq!(IN_FLIGHT.load(Ordering::SeqCst), 0);
}

#[test]
fn console_blocks_raw_and_legacy_motion_and_rebases_on_close() {
    let _lock = INPUT_TEST_LOCK.lock().unwrap();
    type MouseEvent = (u8, u8, [i16; 4], u8);
    static EVENTS: Mutex<Vec<MouseEvent>> = Mutex::new(Vec::new());
    unsafe extern "system" fn feed(
        _: usize,
        button: u8,
        down: u8,
        x: i16,
        y: i16,
        dx: i16,
        dy: i16,
        motion: u8,
    ) {
        EVENTS
            .lock()
            .unwrap()
            .push((button, down, [x, y, dx, dy], motion));
    }
    let old_feed = MOUSE_FEED.swap(feed as *const () as usize, Ordering::AcqRel);
    let mut mouse = [0i16; 32];
    mouse[2..4].copy_from_slice(&[100, 200]);
    mouse[4..6].copy_from_slice(&[7, -8]);
    let address = mouse.as_mut_ptr() as usize;
    let queued = [0u8; 40];
    let begin = queued.as_ptr() as usize;
    unsafe {
        ((address + 0x18) as *mut usize).write_unaligned(begin);
        ((address + 0x20) as *mut usize).write_unaligned(begin + 40);
        ((address + 0x28) as *mut usize).write_unaligned(begin + 40);
        ((address + 0x10) as *mut [u8; 7]).write_unaligned([1; 7]);
    }
    gui::DORMANT.store(false, Ordering::Release);
    gui::MENU_OPEN.store(true, Ordering::Release);
    for button in 0..=6 {
        for motion in 0..=1 {
            unsafe { mouse_feed_hook(address, button, 1, 900, 800, 17, -19, motion) };
        }
    }
    for delta in [120, (-120i8) as u8, 0] {
        unsafe { mouse_feed_hook(address, 4, delta, 900, 800, 0, 0, 0) };
    }
    assert!(EVENTS.lock().unwrap().is_empty());
    assert_eq!(mouse[4..6], [0, 0]);
    assert_eq!(mouse[6..8], [100, 200]);
    assert_eq!(unsafe { read::<usize>(address + 0x20) }, begin);
    assert_eq!(unsafe { read::<[u8; 7]>(address + 0x10) }, [0; 7]);
    assert_eq!(IN_FLIGHT.load(Ordering::SeqCst), 0);

    gui::MENU_OPEN.store(false, Ordering::Release);
    // A wheel packet has no new absolute position; rebase must wait for motion.
    unsafe { mouse_feed_hook(address, 4, 120, 0, 0, 0, 0, 0) };
    assert_eq!(MOUSE_REBASE.load(Ordering::Acquire), address);
    unsafe { mouse_feed_hook(address, 0, 0, 901, 802, 17, -19, 1) };
    assert_eq!(EVENTS.lock().unwrap()[1], (0, 0, [901, 802, 0, 0], 1));
    assert_eq!(mouse[2..4], [901, 802]);
    assert_eq!(mouse[6..8], [901, 802]);
    unsafe { mouse_feed_hook(address, 0, 0, 905, 799, 4, -3, 0) };
    assert_eq!(EVENTS.lock().unwrap()[2], (0, 0, [905, 799, 4, -3], 0));
    assert_eq!(MOUSE_REBASE.load(Ordering::Acquire), 0);
    for delta in [120, (-120i8) as u8] {
        unsafe { mouse_feed_hook(address, 4, delta, 905, 799, 0, 0, 0) };
    }
    assert_eq!(EVENTS.lock().unwrap()[3], (4, 120, [905, 799, 0, 0], 0));
    assert_eq!(
        EVENTS.lock().unwrap()[4],
        (4, (-120i8) as u8, [905, 799, 0, 0], 0)
    );
    gui::MENU_OPEN.store(true, Ordering::Release);
    gui::DORMANT.store(true, Ordering::Release);
    unsafe { mouse_feed_hook(address, 0, 0, 910, 811, 5, 12, 0) };
    assert_eq!(EVENTS.lock().unwrap()[5], (0, 0, [910, 811, 5, 12], 0));
    gui::MENU_OPEN.store(false, Ordering::Release);
    gui::DORMANT.store(false, Ordering::Release);
    MOUSE_FEED.store(old_feed, Ordering::Release);
    assert_eq!(IN_FLIGHT.load(Ordering::SeqCst), 0);
}

#[test]
fn forward_without_back_or_sneak() {
    for forward in [1 << 13, 1 << 9, 1 << 10] {
        assert!(wants_sprint(forward));
        assert!(wants_sprint(forward | (1 << 8) | (1 << 7)));
        assert!(!wants_sprint(forward | 1));
        assert!(!wants_sprint(forward | (1 << 14)));
    }
    assert!(!wants_sprint(0));
    assert!(!wants_sprint(1 << 15));
}

#[test]
fn attack_helper_preserves_all_arguments_and_native_result() {
    let _lock = INPUT_TEST_LOCK.lock().unwrap();
    use super::super::weapon;
    static ARGS: Mutex<Option<(usize, usize, u8, usize)>> = Mutex::new(None);
    unsafe extern "system" fn original(
        mode: usize,
        target: usize,
        flag: u8,
        point: *const [f32; 3],
    ) -> u8 {
        *ARGS.lock().unwrap() = Some((mode, target, flag, point as usize));
        0xa5
    }
    let previous = weapon::ORIGINAL.swap(original as *const () as usize, Ordering::AcqRel);
    let point = [1., 2., 3.];
    let result = unsafe { weapon::attack_hook(0, 0x1234, 7, &point) };
    weapon::ORIGINAL.store(previous, Ordering::Release);
    assert_eq!(result, 0xa5);
    assert_eq!(
        *ARGS.lock().unwrap(),
        Some((0, 0x1234, 7, &point as *const _ as usize))
    );
    assert_eq!(IN_FLIGHT.load(Ordering::SeqCst), 0);
}
#[test]
fn only_owns_automatic_movement_input() {
    // A vanilla reset can clear an input that we still own. It must be
    // pressed again when the automatic request is still active.
    assert_eq!(button_change(true, false, true, false), ButtonChange::Press);
    assert_eq!(
        button_change(true, false, false, false),
        ButtonChange::Press
    );
    assert_eq!(
        button_change(false, false, true, true),
        ButtonChange::Release
    );
    assert_eq!(button_change(false, false, false, true), ButtonChange::None);
    assert_eq!(button_change(false, true, true, true), ButtonChange::None);
    assert_eq!(button_change(true, false, true, true), ButtonChange::None);
    assert_eq!(button_change(false, true, false, false), ButtonChange::None);
}
#[test]
fn automatic_jump_releases_on_takeoff_and_restarts_on_landing() {
    let jump = jump_demand(1 << 13, false, true, false, true, false);
    assert_eq!(
        button_change(jump.wants_input(true, false), false, false, false),
        ButtonChange::Press
    );
    assert_eq!(
        button_change(jump.wants_input(false, false), false, true, true),
        ButtonChange::Release
    );
    assert_eq!(
        button_change(jump.wants_input(false, false), false, false, false),
        ButtonChange::None
    );
    assert_eq!(
        button_change(jump.wants_input(true, false), false, false, false),
        ButtonChange::Press
    );
    // Physical Space remains pressed even when an automatic hold preceded it.
    assert_eq!(
        button_change(jump.wants_input(false, false), true, true, true),
        ButtonChange::None
    );
}
#[test]
fn repeats_after_interval_without_bursts_and_resets_on_repress() {
    let mut clock = ClickClock { next: 0, epoch: 0 };
    assert!(!clock.due(100, 50, 1));
    assert!(!clock.due(149, 50, 1));
    assert!(clock.due(150, 50, 1));
    assert!(clock.due(1000, 50, 1));
    assert!(!clock.due(1000, 50, 1));
    assert!(!clock.due(1001, 0, 2));
    assert!(!clock.due(1002, 50, 3));
    assert!(clock.due(1052, 50, 3));
    // Release/repress between game frames still restarts the interval.
    assert!(!clock.due(1053, 50, 5));
}
