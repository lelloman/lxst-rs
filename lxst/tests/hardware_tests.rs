use lxst::{Key, KeyTransition, Lcd1602Buffer, MatrixKeypad};

#[test]
fn keypad_4x4_layout_matches_python_driver() {
    let keypad = MatrixKeypad::gpio_4x4();
    assert_eq!(keypad.rows(), 4);
    assert_eq!(keypad.cols(), 4);
    assert_eq!(keypad.key_at(0, 0), Some(Key::Char('1')));
    assert_eq!(keypad.key_at(0, 3), Some(Key::Char('A')));
    assert_eq!(keypad.key_at(3, 0), Some(Key::Char('*')));
    assert_eq!(keypad.key_at(3, 3), Some(Key::Char('D')));
    assert_eq!(MatrixKeypad::GPIO_4X4_HOOK_PIN, 5);
}

#[test]
fn keypad_5x5_layout_matches_python_driver() {
    let keypad = MatrixKeypad::gpio_5x5();
    assert_eq!(keypad.rows(), 5);
    assert_eq!(keypad.cols(), 5);
    assert_eq!(keypad.key_at(0, 0), Some(Key::Char('P')));
    assert_eq!(keypad.key_at(0, 4), Some(Key::Char('+')));
    assert_eq!(keypad.key_at(4, 0), Some(Key::Char('*')));
    assert_eq!(keypad.key_at(4, 4), Some(Key::Char('K')));
    assert_eq!(MatrixKeypad::GPIO_5X5_HOOK_PIN, 11);
}

#[test]
fn keypad_reports_down_and_up_transitions_in_map_order() {
    let mut keypad = MatrixKeypad::gpio_4x4().with_hook();
    let events = keypad.update_active_keys([Key::Char('2'), Key::Char('1'), Key::Hook]);
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].key, Key::Char('1'));
    assert_eq!(events[0].transition, KeyTransition::Down);
    assert_eq!(events[1].key, Key::Char('2'));
    assert_eq!(events[1].transition, KeyTransition::Down);
    assert_eq!(events[2].key, Key::Hook);
    assert!(keypad.is_down(Key::Hook));

    let events = keypad.update_active_keys([Key::Char('2')]);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].key, Key::Char('1'));
    assert_eq!(events[0].transition, KeyTransition::Up);
    assert_eq!(events[1].key, Key::Hook);
    assert_eq!(events[1].transition, KeyTransition::Up);
}

#[test]
fn keypad_scan_maps_active_rows_and_columns_to_events() {
    let mut keypad = MatrixKeypad::gpio_4x4();

    let events = keypad.scan_matrix_at(|row, col| (row, col) == (1, 2), None, 1_000);

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].key, Key::Char('6'));
    assert_eq!(events[0].transition, KeyTransition::Down);
    assert!(keypad.is_down(Key::Char('6')));

    let events = keypad.scan_matrix_at(|_, _| false, None, 1_020);

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].key, Key::Char('6'));
    assert_eq!(events[0].transition, KeyTransition::Up);
}

#[test]
fn keypad_scan_keeps_hook_active_during_debounce_window() {
    let mut keypad = MatrixKeypad::gpio_4x4().with_hook();

    let events = keypad.scan_matrix_at(|_, _| false, Some(true), 1_000);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].key, Key::Hook);
    assert_eq!(events[0].transition, KeyTransition::Down);

    let events = keypad.scan_matrix_at(|_, _| false, Some(false), 1_100);
    assert!(events.is_empty());
    assert!(keypad.is_down(Key::Hook));

    let events = keypad.scan_matrix_at(|_, _| false, Some(false), 1_151);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].key, Key::Hook);
    assert_eq!(events[0].transition, KeyTransition::Up);
}

#[test]
fn keypad_ignores_scans_with_too_many_active_keys() {
    let mut keypad = MatrixKeypad::gpio_5x5();
    let events = keypad.update_active_keys([
        Key::Char('1'),
        Key::Char('2'),
        Key::Char('3'),
        Key::Char('A'),
        Key::Char('B'),
    ]);
    assert!(events.is_empty());
    assert!(keypad.is_up(Key::Char('1')));
}

#[test]
fn lcd1602_buffer_print_clear_sleep_and_wake_match_driver_shape() {
    let mut lcd = Lcd1602Buffer::new();
    lcd.print("Telephone Ready", 0, 0);
    lcd.print("42", 14, 1);
    assert_eq!(lcd.row(0), Some("Telephone Ready "));
    assert_eq!(lcd.row(1), Some("              42"));

    lcd.sleep();
    assert!(lcd.is_sleeping());
    assert_eq!(lcd.row(0), Some("                "));

    lcd.print("Wake", 0, 0);
    assert!(!lcd.is_sleeping());
    assert_eq!(lcd.row(0), Some("Wake            "));

    lcd.clear();
    assert_eq!(lcd.row(0), Some("                "));
    assert_eq!(lcd.row(1), Some("                "));
}
