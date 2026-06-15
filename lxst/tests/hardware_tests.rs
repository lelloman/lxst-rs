use lxst::{
    BufferedLcd1602, Key, KeyTransition, Lcd1602Buffer, Lcd1602Display, MatrixKeypad,
    MatrixKeypadBackend, MatrixKeypadScanner,
};

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
fn keypad_scanner_polls_backend_into_key_events() {
    let mut scanner = MatrixKeypadScanner::new(
        MatrixKeypad::gpio_4x4(),
        FakeKeypadBackend::with_active([(2, 1)]),
    );

    let events = scanner.poll_at(500);

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].key, Key::Char('8'));
    assert_eq!(events[0].transition, KeyTransition::Down);
    assert_eq!(scanner.backend().reads.len(), 16);

    scanner.backend_mut().active.clear();
    let events = scanner.poll_at(520);

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].key, Key::Char('8'));
    assert_eq!(events[0].transition, KeyTransition::Up);
}

#[test]
fn keypad_scanner_reads_hook_state_from_backend() {
    let mut scanner = MatrixKeypadScanner::new(
        MatrixKeypad::gpio_4x4().with_hook(),
        FakeKeypadBackend {
            hook_on: Some(true),
            ..FakeKeypadBackend::default()
        },
    );

    let events = scanner.poll_at(1_000);

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].key, Key::Hook);
    assert_eq!(events[0].transition, KeyTransition::Down);

    scanner.backend_mut().hook_on = Some(false);
    let events = scanner.poll_at(1_200);

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

#[test]
fn lcd1602_buffer_implements_display_backend_contract() {
    let mut display: Box<dyn Lcd1602Display> = Box::new(Lcd1602Buffer::new());

    display.print("Ready", 0, 0);
    display.sleep();
    assert!(display.is_sleeping());
    display.wake();
    assert!(!display.is_sleeping());
    display.clear();
}

#[test]
fn buffered_lcd1602_exposes_buffered_backend_for_tests_and_platform_adapters() {
    let mut display = BufferedLcd1602::new();

    drive_ready_display(&mut display);

    assert_eq!(display.buffer().row(0), Some("Telephone Ready "));
    assert_eq!(display.buffer().row(1), Some("                "));

    display.sleep();
    assert!(display.is_sleeping());
    assert!(display.buffer().is_sleeping());

    display.print("Wake", 0, 0);
    assert_eq!(display.buffer().row(0), Some("Wake            "));
    assert!(!display.is_sleeping());
}

fn drive_ready_display(display: &mut dyn Lcd1602Display) {
    display.clear();
    display.print("Telephone Ready", 0, 0);
    display.print("", 0, 1);
}

#[derive(Debug, Default)]
struct FakeKeypadBackend {
    active: Vec<(usize, usize)>,
    hook_on: Option<bool>,
    reads: Vec<(usize, usize)>,
}

impl FakeKeypadBackend {
    fn with_active(active: impl IntoIterator<Item = (usize, usize)>) -> Self {
        Self {
            active: active.into_iter().collect(),
            hook_on: None,
            reads: Vec::new(),
        }
    }
}

impl MatrixKeypadBackend for FakeKeypadBackend {
    fn read_col(&mut self, row: usize, col: usize) -> bool {
        self.reads.push((row, col));
        self.active.contains(&(row, col))
    }

    fn hook_on(&mut self) -> Option<bool> {
        self.hook_on
    }
}
