use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Char(char),
    Hook,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyTransition {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeypadEvent {
    pub key: Key,
    pub transition: KeyTransition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatrixKeypad {
    rows: usize,
    cols: usize,
    key_map: Vec<Key>,
    key_states: HashMap<Key, bool>,
    max_active_keys: usize,
    hook_last_active_ms: Option<u64>,
}

impl MatrixKeypad {
    pub const GPIO_4X4_ROW_PINS: [u8; 4] = [21, 20, 16, 12];
    pub const GPIO_4X4_COL_PINS: [u8; 4] = [26, 19, 13, 6];
    pub const GPIO_4X4_HOOK_PIN: u8 = 5;
    pub const GPIO_5X5_ROW_PINS: [u8; 5] = [21, 20, 16, 12, 7];
    pub const GPIO_5X5_COL_PINS: [u8; 5] = [26, 19, 13, 6, 5];
    pub const GPIO_5X5_HOOK_PIN: u8 = 11;
    pub const SCAN_INTERVAL_MS: u64 = 20;
    pub const HOOK_DEBOUNCE_MS: u64 = 150;

    pub fn gpio_4x4() -> Self {
        Self::new(
            4,
            4,
            &[
                '1', '2', '3', 'A', '4', '5', '6', 'B', '7', '8', '9', 'C', '*', '0', '#', 'D',
            ],
        )
    }

    pub fn gpio_5x5() -> Self {
        Self::new(
            5,
            5,
            &[
                'P', 'R', 'M', '-', '+', '1', '2', '3', 'A', 'B', '4', '5', '6', 'C', 'D', '7',
                '8', '9', 'E', 'F', '*', '0', '#', 'N', 'K',
            ],
        )
    }

    pub fn new(rows: usize, cols: usize, key_map: &[char]) -> Self {
        assert_eq!(rows * cols, key_map.len(), "invalid keypad dimensions");
        let key_map: Vec<_> = key_map.iter().copied().map(Key::Char).collect();
        let key_states = key_map.iter().map(|key| (*key, false)).collect();
        Self {
            rows,
            cols,
            key_map,
            key_states,
            max_active_keys: 4,
            hook_last_active_ms: None,
        }
    }

    pub fn with_hook(mut self) -> Self {
        if !self.key_states.contains_key(&Key::Hook) {
            self.key_map.push(Key::Hook);
            self.key_states.insert(Key::Hook, false);
        }
        self
    }

    pub fn scan_matrix_at<F>(
        &mut self,
        mut read_col: F,
        hook_on: Option<bool>,
        now_ms: u64,
    ) -> Vec<KeypadEvent>
    where
        F: FnMut(usize, usize) -> bool,
    {
        let mut active_keys = Vec::new();
        for row in 0..self.rows {
            for col in 0..self.cols {
                if read_col(row, col) {
                    if let Some(key) = self.key_at(row, col) {
                        active_keys.push(key);
                    }
                }
            }
        }

        if let Some(hook_on) = hook_on {
            if hook_on {
                active_keys.push(Key::Hook);
                self.hook_last_active_ms = Some(now_ms);
            } else if self.is_down(Key::Hook) {
                let elapsed = self
                    .hook_last_active_ms
                    .map(|last| now_ms.saturating_sub(last))
                    .unwrap_or(u64::MAX);
                if elapsed < Self::HOOK_DEBOUNCE_MS {
                    active_keys.push(Key::Hook);
                } else {
                    self.hook_last_active_ms = Some(now_ms);
                }
            }
        }

        self.update_active_keys(active_keys)
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn key_at(&self, row: usize, col: usize) -> Option<Key> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        self.key_map.get(row * self.cols + col).copied()
    }

    pub fn is_down(&self, key: Key) -> bool {
        self.key_states.get(&key).copied().unwrap_or(false)
    }

    pub fn is_up(&self, key: Key) -> bool {
        !self.is_down(key)
    }

    pub fn update_active_keys<I>(&mut self, active_keys: I) -> Vec<KeypadEvent>
    where
        I: IntoIterator<Item = Key>,
    {
        let active_keys: HashSet<Key> = active_keys.into_iter().collect();
        if active_keys.len() > self.max_active_keys {
            return Vec::new();
        }

        let mut events = Vec::new();
        for key in &self.key_map {
            let is_active = active_keys.contains(key);
            let was_active = self.key_states.get(key).copied().unwrap_or(false);
            match (was_active, is_active) {
                (false, true) => {
                    self.key_states.insert(*key, true);
                    events.push(KeypadEvent {
                        key: *key,
                        transition: KeyTransition::Down,
                    });
                }
                (true, false) => {
                    self.key_states.insert(*key, false);
                    events.push(KeypadEvent {
                        key: *key,
                        transition: KeyTransition::Up,
                    });
                }
                _ => {}
            }
        }
        events
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lcd1602Buffer {
    rows: [String; 2],
    sleeping: bool,
}

impl Default for Lcd1602Buffer {
    fn default() -> Self {
        Self::new()
    }
}

impl Lcd1602Buffer {
    pub const COLS: usize = 16;
    pub const ROWS: usize = 2;
    pub const DEFAULT_ADDR: u8 = 0x27;
    pub const DEFAULT_I2C_CH: u8 = 1;

    pub fn new() -> Self {
        Self {
            rows: [blank_lcd_row(), blank_lcd_row()],
            sleeping: false,
        }
    }

    pub fn print(&mut self, value: &str, x: usize, y: usize) {
        if self.sleeping {
            self.wake();
        }
        let row = y.min(Self::ROWS - 1);
        let col = x.min(Self::COLS - 1);
        let mut chars: Vec<char> = self.rows[row].chars().collect();
        let mut input = value.chars().chain(std::iter::repeat(' '));
        for slot in chars.iter_mut().take(Self::COLS).skip(col) {
            *slot = input.next().unwrap_or(' ');
        }
        self.rows[row] = chars.into_iter().collect();
    }

    pub fn clear(&mut self) {
        self.rows = [blank_lcd_row(), blank_lcd_row()];
    }

    pub fn sleep(&mut self) {
        self.sleeping = true;
        self.clear();
    }

    pub fn wake(&mut self) {
        self.sleeping = false;
        self.clear();
    }

    pub fn is_sleeping(&self) -> bool {
        self.sleeping
    }

    pub fn row(&self, row: usize) -> Option<&str> {
        self.rows.get(row).map(String::as_str)
    }
}

fn blank_lcd_row() -> String {
    " ".repeat(Lcd1602Buffer::COLS)
}

pub trait Lcd1602Display {
    fn print(&mut self, value: &str, x: usize, y: usize);
    fn clear(&mut self);
    fn sleep(&mut self);
    fn wake(&mut self);
    fn is_sleeping(&self) -> bool;
}

impl Lcd1602Display for Lcd1602Buffer {
    fn print(&mut self, value: &str, x: usize, y: usize) {
        Lcd1602Buffer::print(self, value, x, y);
    }

    fn clear(&mut self) {
        Lcd1602Buffer::clear(self);
    }

    fn sleep(&mut self) {
        Lcd1602Buffer::sleep(self);
    }

    fn wake(&mut self) {
        Lcd1602Buffer::wake(self);
    }

    fn is_sleeping(&self) -> bool {
        Lcd1602Buffer::is_sleeping(self)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BufferedLcd1602 {
    buffer: Lcd1602Buffer,
}

impl BufferedLcd1602 {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn buffer(&self) -> &Lcd1602Buffer {
        &self.buffer
    }

    pub fn buffer_mut(&mut self) -> &mut Lcd1602Buffer {
        &mut self.buffer
    }

    pub fn into_buffer(self) -> Lcd1602Buffer {
        self.buffer
    }
}

impl Lcd1602Display for BufferedLcd1602 {
    fn print(&mut self, value: &str, x: usize, y: usize) {
        self.buffer.print(value, x, y);
    }

    fn clear(&mut self) {
        self.buffer.clear();
    }

    fn sleep(&mut self) {
        self.buffer.sleep();
    }

    fn wake(&mut self) {
        self.buffer.wake();
    }

    fn is_sleeping(&self) -> bool {
        self.buffer.is_sleeping()
    }
}
