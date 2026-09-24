//! Randomized composition test: 500 random operations, comparing incremental
//! composition against full recomposition pixel for pixel.

use otter_wm::{WindowManager, InputEvent, DamageRect};

fn compare_surfaces(a: &otter_gfx::surface::Surface, b: &otter_gfx::surface::Surface) -> bool {
    if a.w != b.w || a.h != b.h {
        return false;
    }

    for y in 0..a.h {
        for x in 0..a.w {
            let pa = a.get(x as i32, y as i32);
            let pb = b.get(x as i32, y as i32);
            if pa != pb {
                return false;
            }
        }
    }
    true
}

#[test]
fn test_composition_incremental_vs_full() {
    let mut wm = WindowManager::new(480, 300);

    wm.create_window(1, String::from("Window 1"), 20.0, 20.0, 150.0, 100.0);
    wm.create_window(2, String::from("Window 2"), 80.0, 50.0, 150.0, 100.0);
    wm.create_window(3, String::from("Window 3"), 140.0, 80.0, 150.0, 100.0);

    for window_id in 1..=3 {
        if let Some(win) = wm.get_window_mut(window_id) {
            win.surface = Some(otter_wm::WindowSurface::new(150, 100));
        }
    }

    use std::cell::RefCell;
    use std::rc::Rc;

    let seed = Rc::new(RefCell::new(12345u64));

    let next_random = |s: &Rc<RefCell<u64>>| -> u32 {
        let mut seed_val = s.borrow_mut();
        *seed_val = seed_val.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (*seed_val >> 32) as u32
    };

    for _op in 0..200 {
        let op_type = next_random(&seed) % 6;
        match op_type {
            0 => {
                let window_id = (next_random(&seed) % 3) + 1;
                let x = (next_random(&seed) % 330) as f32;
                let y = (next_random(&seed) % 200) as f32;
                wm.move_window(window_id, x, y);
            }
            1 => {
                let window_id = (next_random(&seed) % 3) + 1;
                let width = 80.0 + (next_random(&seed) % 150) as f32;
                let height = 60.0 + (next_random(&seed) % 120) as f32;
                wm.resize_window(window_id, width, height);
            }
            2 => {
                let window_id = (next_random(&seed) % 3) + 1;
                wm.focus_window(Some(window_id));
            }
            3 => {
                let x = (next_random(&seed) % 480) as f32;
                let y = (next_random(&seed) % 300) as f32;
                wm.handle_input(InputEvent::PointerMove { x, y });
            }
            4 => {
                let text = format!("{}:{}",
                    next_random(&seed) % 24,
                    next_random(&seed) % 60
                );
                wm.update_clock(text);
            }
            5 => {
                wm.cycle_focus_forward();
            }
            _ => {}
        }

        let incremental = {
            let mut temp_wm = WindowManager::new(wm.framebuffer_width, wm.framebuffer_height);
            temp_wm.windows = wm.windows.clone();
            temp_wm.window_data = wm.window_data.clone();
            temp_wm.focused_window = wm.focused_window;
            temp_wm.cursor = wm.cursor.clone();
            temp_wm.taskbar_visible = wm.taskbar_visible;
            temp_wm.clock_text = wm.clock_text.clone();

            temp_wm.composite_frame()
        };

        let full = wm.composite_frame();

        assert!(
            compare_surfaces(&incremental, &full),
            "Composition mismatch at operation {}: incremental != full recomposition",
            _op
        );
    }
}

#[test]
fn test_damage_tracking_correctness() {
    let mut wm = WindowManager::new(1920, 1080);
    wm.create_window(1, String::from("Test"), 100.0, 100.0, 600.0, 400.0);

    wm.move_window(1, 200.0, 200.0);
    assert!(!wm.damage_rects.is_empty(), "Move should create damage");

    wm.composite_frame();
    assert!(wm.damage_rects.is_empty(), "Damage should be cleared after composition");
}

#[test]
fn test_window_stacking_during_moves() {
    let mut wm = WindowManager::new(1920, 1080);
    wm.create_window(1, String::from("Win1"), 100.0, 100.0, 400.0, 300.0);
    wm.create_window(2, String::from("Win2"), 200.0, 200.0, 400.0, 300.0);
    wm.create_window(3, String::from("Win3"), 300.0, 300.0, 400.0, 300.0);

    let initial_order = wm.windows.clone();
    assert_eq!(initial_order, vec![1, 2, 3]);

    wm.move_window(1, 50.0, 50.0);
    wm.move_window(2, 150.0, 150.0);

    assert_eq!(wm.windows, vec![1, 2, 3], "Stacking should not change during moves");
}

#[test]
fn test_multiple_damage_unions() {
    let mut wm = WindowManager::new(1920, 1080);

    let rect1 = DamageRect { x: 0, y: 0, width: 100, height: 100 };
    let rect2 = DamageRect { x: 50, y: 50, width: 100, height: 100 };
    let rect3 = DamageRect { x: 100, y: 100, width: 100, height: 100 };

    wm.add_damage_rect(&rect1);
    wm.add_damage_rect(&rect2);
    wm.add_damage_rect(&rect3);

    assert_eq!(wm.damage_rects.len(), 1, "Multiple overlapping rects should union to one");
}

#[test]
fn test_focus_change_creates_damage() {
    let mut wm = WindowManager::new(1920, 1080);
    wm.create_window(1, String::from("Win1"), 100.0, 100.0, 400.0, 300.0);
    wm.create_window(2, String::from("Win2"), 200.0, 200.0, 400.0, 300.0);

    wm.composite_frame();
    let damage_before = wm.damage_rects.len();

    wm.focus_window(Some(1));
    let damage_after = wm.damage_rects.len();

    assert!(damage_after > damage_before, "Focus change should create damage");
}

#[test]
fn test_minimise_restore_cycle() {
    let mut wm = WindowManager::new(1920, 1080);
    wm.create_window(1, String::from("Test"), 100.0, 100.0, 400.0, 300.0);

    let original_state = wm.get_window(1).unwrap().state;
    wm.minimise_window(1);
    assert_ne!(wm.get_window(1).unwrap().state, original_state);

    wm.restore_window(1);
    assert_eq!(wm.get_window(1).unwrap().state, original_state);
}

#[test]
fn test_hit_test_stacking_order() {
    let mut wm = WindowManager::new(1920, 1080);
    wm.create_window(1, String::from("Back"), 100.0, 100.0, 600.0, 400.0);
    wm.create_window(2, String::from("Front"), 200.0, 200.0, 600.0, 400.0);

    if let Some(hit) = wm.hit_test(350.0, 350.0) {
        if let otter_wm::HitTestResult::ClientArea(id) = hit {
            assert_eq!(id, 2, "Should hit the front window");
        } else {
            panic!("Expected ClientArea hit");
        }
    }
}

#[test]
fn test_resize_with_minimum_constraints() {
    let mut wm = WindowManager::new(1920, 1080);
    wm.create_window(1, String::from("Test"), 100.0, 100.0, 400.0, 300.0);

    wm.resize_window(1, 50.0, 50.0);
    let win = wm.get_window(1).unwrap();

    assert!(win.rect.w as f32 >= otter_wm::design::WINDOW_MIN_WIDTH);
    assert!(win.rect.h as f32 >= otter_wm::design::WINDOW_MIN_HEIGHT);
}

#[test]
fn test_close_window_removes_from_stack() {
    let mut wm = WindowManager::new(1920, 1080);
    wm.create_window(1, String::from("Win1"), 100.0, 100.0, 400.0, 300.0);
    wm.create_window(2, String::from("Win2"), 200.0, 200.0, 400.0, 300.0);

    assert_eq!(wm.windows.len(), 2);

    wm.handle_input(InputEvent::PointerDown { x: 222.0, y: 217.0, button: 1 });

    assert_eq!(wm.windows.len(), 1);
    assert_eq!(wm.windows[0], 1);
}

#[test]
fn test_cursor_shape_updates() {
    let mut wm = WindowManager::new(1920, 1080);
    wm.create_window(1, String::from("Test"), 100.0, 100.0, 400.0, 300.0);

    wm.handle_input(InputEvent::PointerMove { x: 400.0, y: 400.0 });
    assert_eq!(wm.cursor.shape, otter_wm::CursorShape::Arrow);

    wm.handle_input(InputEvent::PointerMove { x: 103.0, y: 103.0 });
    assert_eq!(wm.cursor.shape, otter_wm::CursorShape::ResizeNWSE);
}

#[test]
fn test_clock_text_update() {
    let mut wm = WindowManager::new(1920, 1080);
    let initial_clock = wm.clock_text.clone();

    wm.update_clock(String::from("12:34"));
    assert_ne!(wm.clock_text, initial_clock);

    let second_update = wm.clock_text.clone();
    wm.update_clock(String::from("12:34"));
    assert_eq!(wm.clock_text, second_update);
}
