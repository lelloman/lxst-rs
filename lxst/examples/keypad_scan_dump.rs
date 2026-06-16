use lxst::{Key, MatrixKeypad};

fn main() {
    let mut args = std::env::args().skip(1);
    let row = args.next().and_then(|value| value.parse::<usize>().ok());
    let col = args.next().and_then(|value| value.parse::<usize>().ok());
    let hook_on = args.next().as_deref() == Some("hook");

    let mut keypad = MatrixKeypad::gpio_4x4().with_hook();
    let events = keypad.scan_matrix_at(
        |scan_row, scan_col| row == Some(scan_row) && col == Some(scan_col),
        Some(hook_on),
        0,
    );

    println!("4x4 keypad scan dump");
    println!("rows={} cols={}", keypad.rows(), keypad.cols());
    for row in 0..keypad.rows() {
        for col in 0..keypad.cols() {
            match keypad.key_at(row, col) {
                Some(Key::Char(value)) => print!("{value} "),
                Some(Key::Hook) => print!("H "),
                None => print!("? "),
            }
        }
        println!();
    }
    println!("events={events:?}");
}
