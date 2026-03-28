use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use veil_render::{Cell, TermFrame};

pub struct TermCompositor {
    parser: Arc<Mutex<vt100::Parser>>,
    width: u16,
    height: u16,
}

impl TermCompositor {
    pub fn new() -> Self {
        Self {
            parser: Arc::new(Mutex::new(vt100::Parser::new(24, 80, 0))),
            width: 80,
            height: 24,
        }
    }

    pub fn launch(&mut self, app: &str, width: u16, height: u16) -> Box<dyn Write + Send> {
        self.width = width;
        self.height = height;

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize { rows: height, cols: width, pixel_width: 0, pixel_height: 0 })
            .expect("failed to open PTY");

        let cmd = CommandBuilder::new(app);
        pair.slave.spawn_command(cmd).expect("failed to spawn app");
        *self.parser.lock().unwrap() = vt100::Parser::new(height, width, 0);

        let parser = Arc::clone(&self.parser);
        let mut reader = pair.master.try_clone_reader().expect("failed to clone PTY reader");
        thread::spawn(move || {
            let _keep = pair.slave;
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => { parser.lock().unwrap().process(&buf[..n]); }
                }
            }
        });

        pair.master.take_writer().expect("failed to get PTY writer")
    }

    pub fn capture(&self) -> TermFrame {
        let parser = self.parser.lock().unwrap();
        let screen = parser.screen();
        let mut cells = Vec::with_capacity(self.width as usize * self.height as usize);
        for row in 0..self.height {
            for col in 0..self.width {
                let ch = screen
                    .cell(row, col)
                    .and_then(|c| c.contents().chars().next())
                    .unwrap_or(' ');
                cells.push(Cell { ch, luma: char_luma(ch) });
            }
        }
        TermFrame { cells, width: self.width, height: self.height }
    }
}

impl Default for TermCompositor {
    fn default() -> Self { Self::new() }
}

fn char_luma(c: char) -> u8 {
    match c {
        ' ' | '\0'                                        =>   0,
        '.' | ',' | '\'' | '`'                           =>  30,
        '-' | '_' | ':' | ';' | '~'                      =>  60,
        '!' | 'i' | 'l' | '|' | '/' | '\\'              =>  90,
        '(' | ')' | '[' | ']' | '{' | '}'                =>  95,
        '─' | '│' | '┌' | '┐' | '└' | '┘'              => 110,
        '├' | '┤' | '┬' | '┴' | '┼'                     => 115,
        '═' | '║' | '╔' | '╗' | '╚' | '╝' | '╠' | '╣' => 120,
        '░'                                               =>  60,
        '▒'                                               => 120,
        '▓'                                               => 190,
        '█'                                               => 245,
        '▀' | '▄' | '▌' | '▐'                           => 180,
        c if c.is_ascii_lowercase()                       => 130,
        c if c.is_ascii_digit()                           => 150,
        c if c.is_ascii_uppercase()                       => 170,
        c if c.is_ascii_punctuation()                     =>  80,
        _                                                 => 120,
    }
}
