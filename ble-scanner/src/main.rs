//! BLE scanner: listens for Bluetooth LE advertisements and shows nearby
//! devices on the Presto screen, strongest signal first.

use std::{
    collections::HashMap,
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

use pico_std::{
    bluetooth::{self, Advertisement},
    display::{Display, WIDTH},
};

/// Devices not heard from for this long drop off the list.
const FORGET_AFTER: Duration = Duration::from_secs(20);
const REFRESH: Duration = Duration::from_millis(500);
const LOG_EVERY: Duration = Duration::from_secs(5);

// RGB565 colours.
const BLACK: u16 = 0x0000;
const WHITE: u16 = 0xffff;
const GREY: u16 = 0x8410;
const DARK_GREY: u16 = 0x2104;
const LIGHT_GREY: u16 = 0xc618;
const NAVY: u16 = 0x0010;
const GREEN: u16 = 0x07e0;
const YELLOW: u16 = 0xffe0;
const RED: u16 = 0xf800;

// Layout for the 8x13 font on the 480x480 panel.
const CHAR_W: i32 = 8;
const HEADER_H: usize = 28;
const COLUMNS_Y: i32 = 34;
const LIST_Y: usize = 54;
const ROW_H: usize = 15;
const ROWS: usize = (480 - LIST_Y) / ROW_H;
const RSSI_X: i32 = 8;
const BAR_X: usize = 48;
const BAR_W: usize = 64;
const ADDRESS_X: i32 = 120;
const NAME_X: i32 = 264;
const NAME_CHARS: usize = ((WIDTH as i32 - NAME_X - 8) / CHAR_W) as usize;

#[derive(Clone)]
struct Device {
    address_type: u8,
    /// Exponentially smoothed RSSI in dBm, so the order doesn't jitter.
    rssi: f32,
    name: Option<String>,
    company: Option<u16>,
    last_seen: Instant,
}

static DEVICES: Mutex<Option<HashMap<[u8; 6], Device>>> = Mutex::new(None);

fn main() {
    println!("Presto BLE scanner");

    // Start the radio before the display: BTstack may touch flash during
    // init, which must not happen while core 1 is scanning out.
    bluetooth::start_scan(on_advertisement).expect("failed to start Bluetooth");

    let mut display = Display::start().expect("failed to start display");
    display.fill(BLACK);
    draw_static(&mut display);

    let mut last_log = Instant::now();
    loop {
        let devices = snapshot();
        draw_devices(&mut display, &devices);
        if last_log.elapsed() >= LOG_EVERY {
            last_log = Instant::now();
            log_devices(&devices);
        }
        thread::sleep(REFRESH);
    }
}

/// Runs on the CYW43 task for every advertising or scan-response report.
fn on_advertisement(ad: Advertisement<'_>) {
    let (name, company) = parse_ad(ad.data);
    let now = Instant::now();
    let mut devices = DEVICES.lock().unwrap();
    let device = devices
        .get_or_insert_with(HashMap::new)
        .entry(ad.address)
        .or_insert_with(|| Device {
            address_type: ad.address_type,
            rssi: ad.rssi as f32,
            name: None,
            company: None,
            last_seen: now,
        });
    device.rssi = device.rssi * 0.7 + ad.rssi as f32 * 0.3;
    device.last_seen = now;
    // Names and manufacturer data often arrive in only one of the
    // advertisement / scan response pair, so keep what we have.
    if let Some(name) = name {
        if device.name.as_deref() != Some(name.as_str()) {
            device.name = Some(name);
        }
    }
    if company.is_some() {
        device.company = company;
    }
}

/// Drop stale devices and return the rest, strongest first.
fn snapshot() -> Vec<([u8; 6], Device)> {
    let mut list: Vec<_> = {
        let mut guard = DEVICES.lock().unwrap();
        let Some(devices) = guard.as_mut() else {
            return Vec::new();
        };
        devices.retain(|_, device| device.last_seen.elapsed() < FORGET_AFTER);
        devices.iter().map(|(a, d)| (*a, d.clone())).collect()
    };
    list.sort_by(|a, b| b.1.rssi.total_cmp(&a.1.rssi));
    list
}

/// Extract the local name and manufacturer company id from AD structures.
fn parse_ad(mut data: &[u8]) -> (Option<String>, Option<u16>) {
    let (mut name, mut company) = (None, None);
    while let [len, rest @ ..] = data {
        let len = *len as usize;
        if len == 0 || len > rest.len() {
            break;
        }
        let (kind, value) = (rest[0], &rest[1..len]);
        match kind {
            // Complete local name wins over the shortened one.
            0x09 => name = Some(String::from_utf8_lossy(value).into_owned()),
            0x08 if name.is_none() => {
                name = Some(String::from_utf8_lossy(value).into_owned())
            }
            0xff if value.len() >= 2 => company = Some(u16::from_le_bytes([value[0], value[1]])),
            _ => {}
        }
        data = &rest[len..];
    }
    let name = name
        .map(|n| n.trim_end_matches('\0').trim().to_owned())
        .filter(|n| !n.is_empty());
    (name, company)
}

fn company_name(id: u16) -> Option<&'static str> {
    Some(match id {
        0x0006 => "Microsoft",
        0x004c => "Apple",
        0x0059 => "Nordic",
        0x0075 => "Samsung",
        0x0087 => "Garmin",
        0x00e0 => "Google",
        0x0157 => "Huami",
        0x02e5 => "Espressif",
        0x038f => "Xiaomi",
        _ => return None,
    })
}

fn format_address(address: &[u8; 6]) -> String {
    let [a, b, c, d, e, f] = address;
    format!("{a:02X}:{b:02X}:{c:02X}:{d:02X}:{e:02X}:{f:02X}")
}

/// What to show in the name column, and in which colour.
fn describe(device: &Device) -> (String, u16) {
    if let Some(name) = &device.name {
        return (name.clone(), WHITE);
    }
    let label = match device.company {
        Some(id) => match company_name(id) {
            Some(company) => format!("({company})"),
            None => format!("(company 0x{id:04x})"),
        },
        None if device.address_type == 0 => "(public address)".to_owned(),
        None => String::new(),
    };
    (label, GREY)
}

fn signal_colour(rssi: i32) -> u16 {
    match rssi {
        -60.. => GREEN,
        -80.. => YELLOW,
        _ => RED,
    }
}

fn fill_rect(display: &mut Display, x: usize, y: usize, w: usize, h: usize, colour: u16) {
    let pixels = display.pixels_mut();
    for row in y..y + h {
        pixels[row * WIDTH + x..row * WIDTH + x + w].fill(colour);
    }
}

/// Fit `text` into exactly `width` characters, so redrawing overwrites
/// whatever was there before without clearing (which would flicker).
fn fit(text: &str, width: usize) -> String {
    let mut out: String = text
        .chars()
        .map(|c| if c.is_ascii_graphic() || c == ' ' { c } else { '?' })
        .take(width)
        .collect();
    let len = out.chars().count();
    out.extend(std::iter::repeat_n(' ', width - len));
    out
}

fn draw_static(display: &mut Display) {
    fill_rect(display, 0, 0, WIDTH, HEADER_H, NAVY);
    display.draw_text(8, 8, "Bluetooth LE scanner", WHITE, None);
    display.draw_text(RSSI_X, COLUMNS_Y, "dBm", GREY, None);
    display.draw_text(BAR_X as i32, COLUMNS_Y, "signal", GREY, None);
    display.draw_text(ADDRESS_X, COLUMNS_Y, "address", GREY, None);
    display.draw_text(NAME_X, COLUMNS_Y, "name", GREY, None);
}

fn draw_devices(display: &mut Display, devices: &[([u8; 6], Device)]) {
    let count = format!("{:>12}", format!("{} devices", devices.len()));
    display.draw_text(WIDTH as i32 - 8 - 12 * CHAR_W, 8, &count, WHITE, Some(NAVY));

    for row in 0..ROWS {
        let y = LIST_Y + row * ROW_H;
        let Some((address, device)) = devices.get(row) else {
            fill_rect(display, 0, y, WIDTH, ROW_H, BLACK);
            continue;
        };
        let rssi = device.rssi.round() as i32;
        let colour = signal_colour(rssi);
        display.draw_text(RSSI_X, y as i32, &format!("{rssi:>4}"), colour, Some(BLACK));

        let filled = ((rssi + 100).clamp(0, 70) as usize * BAR_W) / 70;
        fill_rect(display, BAR_X, y + 2, BAR_W, 9, DARK_GREY);
        fill_rect(display, BAR_X, y + 2, filled, 9, colour);

        display.draw_text(ADDRESS_X, y as i32, &format_address(address), LIGHT_GREY, Some(BLACK));
        let (label, colour) = describe(device);
        display.draw_text(NAME_X, y as i32, &fit(&label, NAME_CHARS), colour, Some(BLACK));
    }
}

fn log_devices(devices: &[([u8; 6], Device)]) {
    println!("{} devices", devices.len());
    for (address, device) in devices.iter().take(10) {
        println!(
            "  {:>4} dBm  {}  {}",
            device.rssi.round() as i32,
            format_address(address),
            describe(device).0
        );
    }
}
