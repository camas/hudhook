//! This module contains functions related to processing input events.

use std::ffi::c_void;
use std::mem::size_of;
use std::sync::mpsc;

use imgui::Io;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Input::{
    GetRawInputData, HRAWINPUT, RAWINPUT, RAWINPUTHEADER, RAWKEYBOARD, RAWMOUSE_0_0,
    RID_DEVICE_INFO_TYPE, RID_INPUT, RIM_TYPEKEYBOARD, RIM_TYPEMOUSE,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::renderer::RenderState;

pub type WndProcType =
    unsafe extern "system" fn(hwnd: HWND, umsg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT;

// Replication of the Win32 HIWORD macro.
#[inline]
fn hiword(l: u32) -> u16 {
    ((l >> 16) & 0xffff) as u16
}

pub(crate) enum InputChange {
    MouseDown { index: usize, value: bool },
    KeyDown { index: usize, value: bool },
    MouseWheelScroll { delta: f32 },
    MouseWheelHorizontalScroll { delta: f32 },
    AddInputCharacter { character: char },
    CtrlPressed { value: bool },
    ShiftPressed { value: bool },
    AltPressed { value: bool },
    SuperPressed { value: bool },
}

////////////////////////////////////////////////////////////////////////////////
// Raw input
////////////////////////////////////////////////////////////////////////////////

// Handle raw mouse input events.
//
// Given the RAWINPUT structure, check each possible mouse flag status and
// update the Io object accordingly. Both the key_down indices associated to the
// mouse click (VK_...) and the values in mouse_down are updated.
fn handle_raw_mouse_input(wnd_proc_tx: &mpsc::Sender<InputChange>, raw_mouse: &RAWMOUSE_0_0) {
    let button_flags = raw_mouse.usButtonFlags as u32;

    let has_flag = |flag| button_flags & flag != 0;
    let set_key_down = |VIRTUAL_KEY(index), val| {
        _ = wnd_proc_tx.send(InputChange::KeyDown { index: index as usize, value: val });
    };

    // Check whether any of the mouse buttons was pressed or released.
    if has_flag(RI_MOUSE_LEFT_BUTTON_DOWN) {
        set_key_down(VK_LBUTTON, true);
        _ = wnd_proc_tx.send(InputChange::MouseDown { index: 0, value: true });
    }
    if has_flag(RI_MOUSE_LEFT_BUTTON_UP) {
        set_key_down(VK_LBUTTON, false);
        _ = wnd_proc_tx.send(InputChange::MouseDown { index: 0, value: false });
    }
    if has_flag(RI_MOUSE_RIGHT_BUTTON_DOWN) {
        set_key_down(VK_RBUTTON, true);
        _ = wnd_proc_tx.send(InputChange::MouseDown { index: 1, value: true });
    }
    if has_flag(RI_MOUSE_RIGHT_BUTTON_UP) {
        set_key_down(VK_RBUTTON, false);
        _ = wnd_proc_tx.send(InputChange::MouseDown { index: 1, value: false });
    }
    if has_flag(RI_MOUSE_MIDDLE_BUTTON_DOWN) {
        set_key_down(VK_MBUTTON, true);
        _ = wnd_proc_tx.send(InputChange::MouseDown { index: 2, value: true });
    }
    if has_flag(RI_MOUSE_MIDDLE_BUTTON_UP) {
        set_key_down(VK_MBUTTON, false);
        _ = wnd_proc_tx.send(InputChange::MouseDown { index: 2, value: false });
    }
    if has_flag(RI_MOUSE_BUTTON_4_DOWN) {
        set_key_down(VK_XBUTTON1, true);
        _ = wnd_proc_tx.send(InputChange::MouseDown { index: 3, value: true });
    }
    if has_flag(RI_MOUSE_BUTTON_4_UP) {
        set_key_down(VK_XBUTTON1, false);
        _ = wnd_proc_tx.send(InputChange::MouseDown { index: 3, value: false });
    }
    if has_flag(RI_MOUSE_BUTTON_5_DOWN) {
        set_key_down(VK_XBUTTON2, true);
        _ = wnd_proc_tx.send(InputChange::MouseDown { index: 4, value: true });
    }
    if has_flag(RI_MOUSE_BUTTON_5_UP) {
        set_key_down(VK_XBUTTON2, false);
        _ = wnd_proc_tx.send(InputChange::MouseDown { index: 4, value: false });
    }

    // Apply vertical mouse scroll.
    if button_flags & RI_MOUSE_WHEEL != 0 {
        let wheel_delta = raw_mouse.usButtonData as i16 / WHEEL_DELTA as i16;
        _ = wnd_proc_tx.send(InputChange::MouseWheelScroll { delta: wheel_delta as f32 });
    }

    // Apply horizontal mouse scroll.
    if button_flags & RI_MOUSE_HWHEEL != 0 {
        let wheel_delta = raw_mouse.usButtonData as i16 / WHEEL_DELTA as i16;
        _ = wnd_proc_tx.send(InputChange::MouseWheelHorizontalScroll { delta: wheel_delta as f32 });
    }
}

// Handle raw keyboard input.
fn handle_raw_keyboard_input(wnd_proc_tx: &mpsc::Sender<InputChange>, raw_keyboard: &RAWKEYBOARD) {
    // Ignore messages without a valid key code
    if raw_keyboard.VKey == 0 {
        return;
    }

    // Extract the keyboard flags.
    let flags = raw_keyboard.Flags as u32;

    // Compute the scan code, applying the prefix if it is present.
    let scan_code = {
        let mut code = raw_keyboard.MakeCode as u32;
        // Necessary to check LEFT/RIGHT keys on CTRL & ALT & others (not shift)
        if flags & RI_KEY_E0 != 0 {
            code |= 0xe000;
        }
        if flags & RI_KEY_E1 != 0 {
            code |= 0xe100;
        }
        code
    };

    // Check the key status.
    let is_key_down = flags == RI_KEY_MAKE;
    let is_key_up = flags & RI_KEY_BREAK != 0;

    // Map the virtual key if necessary.
    let virtual_key = match VIRTUAL_KEY(raw_keyboard.VKey) {
        virtual_key @ (VK_SHIFT | VK_CONTROL | VK_MENU) => {
            match unsafe { MapVirtualKeyA(scan_code, MAPVK_VSC_TO_VK_EX) } {
                0 => virtual_key.0,
                i => i as u16,
            }
        },
        VIRTUAL_KEY(virtual_key) => virtual_key,
    } as usize;

    // If the virtual key is in the allowed array range, set the appropriate status
    // of key_down for that virtual key.
    if virtual_key < 0xFF {
        if is_key_down {
            _ = wnd_proc_tx.send(InputChange::KeyDown { index: virtual_key, value: true });
        }
        if is_key_up {
            _ = wnd_proc_tx.send(InputChange::KeyDown { index: virtual_key, value: false });
        }
    }
}

// Handle WM_INPUT events.
fn handle_raw_input(
    wnd_proc_tx: &mpsc::Sender<InputChange>,
    WPARAM(wparam): WPARAM,
    LPARAM(lparam): LPARAM,
) {
    let mut raw_data = RAWINPUT { ..Default::default() };
    let mut raw_data_size = size_of::<RAWINPUT>() as u32;
    let raw_data_header_size = size_of::<RAWINPUTHEADER>() as u32;

    // Read the raw input data.
    let r = unsafe {
        GetRawInputData(
            HRAWINPUT(lparam),
            RID_INPUT,
            Some(&mut raw_data as *mut _ as *mut c_void),
            &mut raw_data_size,
            raw_data_header_size,
        )
    };

    // If GetRawInputData errors out, return false.
    if r == u32::MAX {
        return;
    }

    // Ignore messages when window is not focused.
    if (wparam as u32 & 0xFFu32) != RIM_INPUT {
        return;
    }

    // Dispatch to the appropriate raw input processing method.
    match RID_DEVICE_INFO_TYPE(raw_data.header.dwType) {
        RIM_TYPEMOUSE => {
            handle_raw_mouse_input(wnd_proc_tx, unsafe {
                &raw_data.data.mouse.Anonymous.Anonymous
            });
        },
        RIM_TYPEKEYBOARD => {
            handle_raw_keyboard_input(wnd_proc_tx, unsafe { &raw_data.data.keyboard });
        },
        _ => {},
    }
}

////////////////////////////////////////////////////////////////////////////////
// Regular input
////////////////////////////////////////////////////////////////////////////////

fn map_vkey(wparam: u16, lparam: usize) -> VIRTUAL_KEY {
    match VIRTUAL_KEY(wparam) {
        VK_SHIFT => unsafe {
            match MapVirtualKeyA(((lparam & 0x00ff0000) >> 16) as u32, MAPVK_VSC_TO_VK_EX) {
                0 => VIRTUAL_KEY(wparam),
                i => VIRTUAL_KEY(i as _),
            }
        },
        VK_CONTROL => {
            if lparam & 0x01000000 != 0 {
                VK_RCONTROL
            } else {
                VK_LCONTROL
            }
        },
        VK_MENU => {
            if lparam & 0x01000000 != 0 {
                VK_RMENU
            } else {
                VK_LMENU
            }
        },
        _ => VIRTUAL_KEY(wparam),
    }
}

// Handle WM_(SYS)KEYDOWN/WM_(SYS)KEYUP events.
fn handle_input(
    wnd_proc_tx: &mpsc::Sender<InputChange>,
    state: u32,
    WPARAM(wparam): WPARAM,
    LPARAM(lparam): LPARAM,
) {
    let pressed = (state == WM_KEYDOWN) || (state == WM_SYSKEYDOWN);
    let key_pressed = map_vkey(wparam as _, lparam as _);
    _ = wnd_proc_tx.send(InputChange::KeyDown { index: key_pressed.0 as usize, value: pressed });

    // According to the winit implementation [1], it's ok to check twice, and the
    // logic isn't flawed either.
    //
    // [1] https://github.com/imgui-rs/imgui-rs/blob/b1e66d050e84dbb2120001d16ce59d15ef6b5303/imgui-winit-support/src/lib.rs#L401-L404
    match key_pressed {
        VK_CONTROL | VK_LCONTROL | VK_RCONTROL => {
            _ = wnd_proc_tx.send(InputChange::CtrlPressed { value: pressed })
        },
        VK_SHIFT | VK_LSHIFT | VK_RSHIFT => {
            _ = wnd_proc_tx.send(InputChange::ShiftPressed { value: pressed })
        },
        VK_MENU | VK_LMENU | VK_RMENU => {
            _ = wnd_proc_tx.send(InputChange::AltPressed { value: pressed })
        },
        VK_LWIN | VK_RWIN => _ = wnd_proc_tx.send(InputChange::SuperPressed { value: pressed }),
        _ => (),
    };
}

pub(crate) fn update_io(io: &mut Io, input_change: InputChange) {
    match input_change {
        InputChange::MouseDown { index, value } => io.mouse_down[index] = value,
        InputChange::KeyDown { index, value } => io.keys_down[index] = value,
        InputChange::MouseWheelScroll { delta } => io.mouse_wheel += delta,
        InputChange::MouseWheelHorizontalScroll { delta } => io.mouse_wheel_h += delta,
        InputChange::AddInputCharacter { character } => io.add_input_character(character),
        InputChange::CtrlPressed { value } => io.key_ctrl = value,
        InputChange::ShiftPressed { value } => io.key_shift = value,
        InputChange::AltPressed { value } => io.key_alt = value,
        InputChange::SuperPressed { value } => io.key_super = value,
    }
}

////////////////////////////////////////////////////////////////////////////////
// Window procedure
////////////////////////////////////////////////////////////////////////////////

#[must_use]
pub fn imgui_wnd_proc_impl(
    hwnd: HWND,
    umsg: u32,
    WPARAM(wparam): WPARAM,
    LPARAM(lparam): LPARAM,
    wnd_proc: WndProcType,
    wnd_proc_tx: &mpsc::Sender<InputChange>,
    should_block_messages: bool,
) -> LRESULT {
    match umsg {
        WM_INPUT => handle_raw_input(wnd_proc_tx, WPARAM(wparam), LPARAM(lparam)),
        state @ (WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP) if wparam < 256 => {
            handle_input(wnd_proc_tx, state, WPARAM(wparam), LPARAM(lparam))
        },
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            _ = wnd_proc_tx.send(InputChange::MouseDown { index: 0, value: true });
        },
        WM_RBUTTONDOWN | WM_RBUTTONDBLCLK => {
            _ = wnd_proc_tx.send(InputChange::MouseDown { index: 1, value: true });
        },
        WM_MBUTTONDOWN | WM_MBUTTONDBLCLK => {
            _ = wnd_proc_tx.send(InputChange::MouseDown { index: 2, value: true });
        },
        WM_XBUTTONDOWN | WM_XBUTTONDBLCLK => {
            let btn = if hiword(wparam as _) == XBUTTON1 { 3 } else { 4 };
            _ = wnd_proc_tx.send(InputChange::MouseDown { index: btn, value: true });
        },
        WM_LBUTTONUP => {
            _ = wnd_proc_tx.send(InputChange::MouseDown { index: 0, value: false });
        },
        WM_RBUTTONUP => {
            _ = wnd_proc_tx.send(InputChange::MouseDown { index: 1, value: false });
        },
        WM_MBUTTONUP => {
            _ = wnd_proc_tx.send(InputChange::MouseDown { index: 2, value: false });
        },
        WM_XBUTTONUP => {
            let btn = if hiword(wparam as _) == XBUTTON1 { 3 } else { 4 };
            _ = wnd_proc_tx.send(InputChange::MouseDown { index: btn, value: false });
        },
        WM_MOUSEWHEEL => {
            // This `hiword` call is equivalent to GET_WHEEL_DELTA_WPARAM
            let wheel_delta_wparam = hiword(wparam as _);
            let wheel_delta = WHEEL_DELTA as f32;
            _ = wnd_proc_tx.send(InputChange::MouseWheelScroll {
                delta: (wheel_delta_wparam as i16 as f32) / wheel_delta,
            });
        },
        WM_MOUSEHWHEEL => {
            // This `hiword` call is equivalent to GET_WHEEL_DELTA_WPARAM
            let wheel_delta_wparam = hiword(wparam as _);
            let wheel_delta = WHEEL_DELTA as f32;
            _ = wnd_proc_tx.send(InputChange::MouseWheelHorizontalScroll {
                delta: (wheel_delta_wparam as i16 as f32) / wheel_delta,
            });
        },
        WM_CHAR => {
            _ = wnd_proc_tx
                .send(InputChange::AddInputCharacter { character: wparam as u8 as char });
        },
        WM_SIZE => {
            RenderState::resize();
            return LRESULT(1);
        },
        _ => {},
    };

    if should_block_messages {
        return LRESULT(1);
    }

    unsafe { CallWindowProcW(Some(wnd_proc), hwnd, umsg, WPARAM(wparam), LPARAM(lparam)) }
}
