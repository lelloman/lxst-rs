use lxst::Lcd1602Buffer;

fn main() {
    let mut args = std::env::args().skip(1);
    let line1 = args.next().unwrap_or_else(|| "LXST validation".to_string());
    let line2 = args.next().unwrap_or_else(|| "LCD1602 OK".to_string());

    let mut display = Lcd1602Buffer::new();
    display.print(&line1, 0, 0);
    display.print(&line2, 0, 1);

    println!("|{}|", display.row(0).unwrap());
    println!("|{}|", display.row(1).unwrap());
}
