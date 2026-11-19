//! Установка иконки окна через Win32.
//!
//! `egui::ViewportCommand::Icon` в eframe 0.37 до окна не доходит (заголовок
//! остаётся с иконкой, заданной при создании), а WM_SETICON обновляет значок
//! сразу. Значок ставим из готового `.ico` набора: берём из файла запись
//! нужного размера и отдаём её `CreateIconFromResourceEx` — Windows создаёт
//! правильный по размеру HICON, поэтому значок остаётся чётким при любом
//! числе переключений темы.

/// Запись одного изображения внутри `.ico`-файла.
struct IcoEntry<'a> {
    width: u32,
    data: &'a [u8],
}

/// Разбирает `.ico` и отдаёт изображение, ближайшее к нужному размеру.
fn pick_entry<'a>(ico: &'a [u8], want: u32) -> Option<IcoEntry<'a>> {
    if ico.len() < 6 {
        return None;
    }
    let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
    let mut best: Option<IcoEntry<'a>> = None;
    let mut best_score = u32::MAX;
    for i in 0..count {
        let at = 6 + i * 16;
        if at + 16 > ico.len() {
            return None;
        }
        let e = &ico[at..at + 16];
        let width = if e[0] == 0 { 256 } else { e[0] as u32 };
        let size = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as usize;
        let offset = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as usize;
        if offset + size > ico.len() || size == 0 {
            continue;
        }
        // Точное попадание лучше всего; иначе — ближайший больший размер,
        // чтобы уменьшение шло с запасом, а не с растягиванием.
        let score = if width == want {
            0
        } else if width > want {
            width - want
        } else {
            1000 - width
        };
        if score < best_score {
            best_score = score;
            best = Some(IcoEntry {
                width,
                data: &ico[offset..offset + size],
            });
        }
    }
    best
}

#[cfg(windows)]
pub fn set(hwnd: isize, ico: &[u8]) {
    use std::sync::Mutex;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateIconFromResourceEx, DestroyIcon, GetSystemMetrics, SendMessageW, ICON_BIG, ICON_SMALL,
        SM_CXICON, SM_CXSMICON, WM_SETICON,
    };

    // Предыдущие HICON нужно освобождать, иначе за сотни переключений
    // темы копится заметный объём GDI-объектов.
    static LAST: Mutex<[isize; 2]> = Mutex::new([0; 2]);

    let big_size = unsafe { GetSystemMetrics(SM_CXICON) } as u32;
    let small_size = unsafe { GetSystemMetrics(SM_CXSMICON) } as u32;
    let Some(big_entry) = pick_entry(ico, big_size) else {
        return;
    };
    let Some(small_entry) = pick_entry(ico, small_size) else {
        return;
    };

    let create = |entry: &IcoEntry| unsafe {
        CreateIconFromResourceEx(
            entry.data.as_ptr(),
            entry.data.len() as u32,
            1,
            0x0003_0000,
            entry.width as i32,
            entry.width as i32,
            0,
        )
    };
    let big = create(&big_entry);
    let small = create(&small_entry);
    if big.is_null() || small.is_null() {
        unsafe {
            if !big.is_null() {
                DestroyIcon(big);
            }
            if !small.is_null() {
                DestroyIcon(small);
            }
        }
        return;
    }

    let hwnd = hwnd as *mut core::ffi::c_void;
    unsafe {
        let mut last = match LAST.lock() {
            Ok(l) => l,
            Err(p) => p.into_inner(),
        };
        SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, small as isize);
        SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, big as isize);
        for (old, new) in last.iter_mut().zip([small as isize, big as isize]) {
            let old = std::mem::replace(old, new);
            if old != 0 {
                DestroyIcon(old as *mut core::ffi::c_void);
            }
        }
    }
}

#[cfg(not(windows))]
pub fn set(_hwnd: isize, _ico: &[u8]) {}
