//! 键位、鼠标、过滤、选择与挂载类操作。

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use std::thread;

use super::Msg;
use crate::model::*;
use crate::mount;
use crate::ops;
use crate::theme::Palette;

use super::{App, Confirm};

impl App {
    pub(crate) fn select_first(&mut self) {
        if let Some(position) = self.rows.iter().position(|row| row.device.is_some()) {
            self.selected = position;
        }
        self.sync_selection();
    }

    pub(crate) fn sync_selection(&mut self) {
        let key = self.selected_peripheral().map(Peripheral::key);
        if key == self.selected_key {
            return;
        }
        self.selected_key = key;
        self.detail_scroll = 0;
        self.dirty = true;
    }

    pub(crate) fn select(&mut self, index: usize) {
        if index < self.rows.len() && self.rows[index].device.is_some() {
            self.selected = index;
            self.sync_selection();
        }
    }

    pub(crate) fn move_by(&mut self, delta: isize) {
        let len = self.rows.len() as isize;
        let mut index = self.selected as isize;
        for _ in 0..len {
            index += delta;
            if index < 0 || index >= len {
                break;
            }
            if self.rows[index as usize].device.is_some() {
                self.select(index as usize);
                return;
            }
        }
    }

    pub(crate) fn move_to_edge(&mut self, last: bool) {
        let position = if last {
            self.rows.iter().rposition(|row| row.device.is_some())
        } else {
            self.rows.iter().position(|row| row.device.is_some())
        };
        if let Some(position) = position {
            self.select(position);
        }
    }

    /// 跳到上一/下一分类的第一个设备。
    pub(crate) fn jump_category(&mut self, delta: isize) {
        let headers: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.device.is_none())
            .map(|(index, _)| index)
            .collect();
        if headers.is_empty() {
            return;
        }
        let current = headers
            .iter()
            .filter(|index| **index <= self.selected)
            .count()
            .saturating_sub(1);
        let target = (current as isize + delta).clamp(0, headers.len() as isize - 1) as usize;
        let start = headers[target];
        if let Some(position) = self.rows[start..]
            .iter()
            .position(|row| row.device.is_some())
        {
            self.select(start + position);
        }
    }

    pub(crate) fn begin_filter(&mut self) {
        self.filter_editing = true;
        self.dirty = true;
        self.set_status("输入关键字过滤，Enter 确认，Esc 清空", false);
    }

    pub(crate) fn push_filter(&mut self, ch: char) {
        self.filter.push(ch);
        self.rebuild();
        self.sync_selection();
        self.dirty = true;
    }

    pub(crate) fn filter_backspace(&mut self) {
        self.filter.pop();
        self.rebuild();
        self.sync_selection();
        self.dirty = true;
    }

    pub(crate) fn clear_filter(&mut self) {
        if !self.filter.is_empty() {
            self.filter.clear();
            self.rebuild();
            self.sync_selection();
            self.dirty = true;
            self.set_status("已清除过滤", false);
        }
        self.filter_editing = false;
    }

    pub(crate) fn part_index(&self, peripheral: &Peripheral) -> usize {
        let total: usize = peripheral.disks.iter().map(|d| d.partitions.len()).sum();
        if total == 0 {
            return 0;
        }
        self.part_sel.get(&peripheral.key()).copied().unwrap_or(0) % total
    }

    pub(crate) fn cycle_partition(&mut self, delta: isize) {
        let Some(peripheral) = self.selected_peripheral().cloned() else {
            return;
        };
        let total: usize = peripheral.disks.iter().map(|d| d.partitions.len()).sum();
        if total == 0 {
            self.set_status("该设备没有分区（整盘可直接挂载）", true);
            return;
        }
        let next = (self.part_index(&peripheral) as isize + delta).rem_euclid(total as isize);
        self.part_sel.insert(peripheral.key(), next as usize);
        self.dirty = true;
    }

    pub(crate) fn mount_selected(&mut self) {
        if self.action_pending.is_some() {
            self.set_status("上一个操作还没完成", true);
            return;
        }
        let Some(peripheral) = self.selected_peripheral().cloned() else {
            return;
        };
        let index = self.part_index(&peripheral);
        let Some(target) = ops::mount_target(&peripheral, index) else {
            if ops::partitions(&peripheral)
                .get(index)
                .is_some_and(|p| p.mount.is_some())
                || peripheral
                    .disks
                    .first()
                    .is_some_and(|disk| disk.mount.is_some())
            {
                self.set_status("已经挂载了", true);
            } else {
                self.set_status("该设备不是存储设备", true);
            }
            return;
        };

        let seq = self.start_action(format!("挂载 {} …", target.device));
        let tx = self.tx.clone();
        thread::spawn(move || {
            let (ok, text) = match mount::mount(&target.device, &target.label) {
                Ok(point) => (true, format!("已挂载 {} → {point}", target.device)),
                Err(error) => (false, format!("挂载失败：{error:#}")),
            };
            let _ = tx.send(Msg::Action { seq, ok, text });
        });
    }

    pub(crate) fn unmount_selected(&mut self) {
        if self.action_pending.is_some() {
            self.set_status("上一个操作还没完成", true);
            return;
        }
        let Some(peripheral) = self.selected_peripheral().cloned() else {
            return;
        };
        let Some(device) = ops::unmount_target(&peripheral, self.part_index(&peripheral)) else {
            self.set_status("没有已挂载的分区", true);
            return;
        };

        let seq = self.start_action(format!("卸载 {device} …"));
        let tx = self.tx.clone();
        thread::spawn(move || {
            let (ok, text) = match mount::unmount(&device) {
                Ok(()) => (true, format!("已卸载 {device}")),
                Err(error) => (false, format!("卸载失败：{error:#}")),
            };
            let _ = tx.send(Msg::Action { seq, ok, text });
        });
    }

    pub(crate) fn eject_selected(&mut self) {
        if self.action_pending.is_some() {
            self.set_status("上一个操作还没完成", true);
            return;
        }
        let Some(peripheral) = self.selected_peripheral().cloned() else {
            return;
        };
        let disk = match ops::eject_disk(&peripheral, self.part_index(&peripheral)) {
            Ok(disk) => disk,
            Err(error) => {
                self.set_status(error.to_string(), true);
                return;
            }
        };

        self.modal = Some(Confirm::Eject {
            device: disk.path.clone(),
            label: disk.descriptor(),
        });
        self.set_status("确认弹出？y 确认 · Esc 取消", false);
        self.dirty = true;
    }

    pub(crate) fn confirm(&mut self, confirmed: bool) {
        let Some(modal) = self.modal.take() else {
            return;
        };
        self.dirty = true;
        if !confirmed {
            self.set_status("已取消", false);
            return;
        }
        match modal {
            Confirm::Eject { device, label } => self.perform_eject(device, label),
        }
    }

    pub(crate) fn perform_eject(&mut self, device: String, label: String) {
        let seq = self.start_action(format!("弹出 {label} …"));
        let tx = self.tx.clone();
        thread::spawn(move || {
            let (ok, text) = match mount::power_off(&device) {
                Ok(()) => (true, format!("已安全弹出 {device}，可以拔线")),
                Err(error) => (false, format!("弹出失败：{error:#}")),
            };
            let _ = tx.send(Msg::Action { seq, ok, text });
        });
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        let code = key.code;
        let modifiers = key.modifiers;
        self.dirty = true;

        if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
            self.quitting = true;
            return;
        }

        if self.modal.is_some() {
            match code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => self.confirm(true),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Char('q') => {
                    self.confirm(false)
                }
                _ => {}
            }
            return;
        }

        if self.show_help {
            match code {
                KeyCode::PageDown | KeyCode::Char('j') | KeyCode::Down => {
                    self.help_scroll = self.help_scroll.saturating_add(3)
                }
                KeyCode::PageUp | KeyCode::Char('k') | KeyCode::Up => {
                    self.help_scroll = self.help_scroll.saturating_sub(3)
                }
                _ => {
                    self.show_help = false;
                    self.help_scroll = 0;
                }
            }
            return;
        }

        if self.filter_editing {
            match code {
                KeyCode::Enter => {
                    self.filter_editing = false;
                    self.set_status(
                        if self.filter.is_empty() {
                            "过滤已结束".to_string()
                        } else {
                            format!("过滤：{}", self.filter)
                        },
                        false,
                    );
                }
                KeyCode::Esc => {
                    if self.filter.is_empty() {
                        self.filter_editing = false;
                    } else {
                        self.clear_filter();
                    }
                }
                KeyCode::Backspace => {
                    if self.filter.is_empty() {
                        self.filter_editing = false;
                    } else {
                        self.filter_backspace();
                    }
                }
                KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => {
                    self.clear_filter();
                }
                KeyCode::Up => self.move_by(-1),
                KeyCode::Down => self.move_by(1),
                KeyCode::Char(ch) if !modifiers.contains(KeyModifiers::CONTROL) => {
                    self.push_filter(ch)
                }
                _ => {}
            }
            return;
        }

        match code {
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::Home | KeyCode::Char('g') => self.move_to_edge(false),
            KeyCode::End | KeyCode::Char('G') => self.move_to_edge(true),
            KeyCode::Tab => self.jump_category(1),
            KeyCode::BackTab => self.jump_category(-1),
            KeyCode::Left | KeyCode::Char('h') => self.cycle_partition(-1),
            KeyCode::Right | KeyCode::Char('l') => self.cycle_partition(1),
            KeyCode::Char('d') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_details(10)
            }
            KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_details(-10)
            }
            KeyCode::Char('m') => self.mount_selected(),
            KeyCode::Char('u') => self.unmount_selected(),
            KeyCode::Char('e') => self.eject_selected(),
            KeyCode::PageDown => self.scroll_details(5),
            KeyCode::PageUp => self.scroll_details(-5),
            KeyCode::Char('J') => self.scroll_details(1),
            KeyCode::Char('K') => self.scroll_details(-1),
            KeyCode::Char('/') => self.begin_filter(),
            KeyCode::Char('r') => {
                self.request_scan();
                self.set_status("正在刷新设备…", false);
            }
            KeyCode::Char('t') => self.cycle_theme(),
            KeyCode::Char('i') => {
                self.show_internal = !self.show_internal;
                self.rebuild();
                self.sync_selection();
                let text = if self.show_internal {
                    "显示内置设备"
                } else {
                    "隐藏内置设备"
                };
                self.set_status(text, false);
            }
            KeyCode::Char('?') => {
                self.show_help = !self.show_help;
                self.help_scroll = 0;
            }
            KeyCode::Esc => {
                if self.filter.is_empty() {
                    self.quitting = true;
                } else {
                    self.clear_filter();
                }
            }
            KeyCode::Char('q') => self.quitting = true,
            _ => {}
        }
    }

    pub fn on_mouse(&mut self, mouse: ratatui::crossterm::event::MouseEvent) {
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};
        let inside = |area: ratatui::layout::Rect, column: u16, row: u16| {
            column >= area.x
                && column < area.x + area.width
                && row >= area.y
                && row < area.y + area.height
        };
        match mouse.kind {
            MouseEventKind::ScrollDown => {
                if inside(self.details_area, mouse.column, mouse.row) {
                    self.scroll_details(3);
                } else {
                    self.move_by(1);
                }
            }
            MouseEventKind::ScrollUp => {
                if inside(self.details_area, mouse.column, mouse.row) {
                    self.scroll_details(-3);
                } else {
                    self.move_by(-1);
                }
            }
            MouseEventKind::Down(MouseButton::Left)
                if inside(self.list_area, mouse.column, mouse.row) =>
            {
                // 去掉上边框，再按列表滚动偏移换算成行号。
                let offset = mouse.row.saturating_sub(self.list_area.y + 1) as usize;
                let index = self.list_offset + offset;
                if index < self.rows.len() && self.rows[index].device.is_some() {
                    self.select(index);
                }
            }
            _ => {}
        }
    }

    pub fn scroll_details(&mut self, delta: i32) {
        let max = self.detail_max;
        let next = (self.detail_scroll as i32 + delta).clamp(0, max as i32) as u16;
        if next != self.detail_scroll {
            self.detail_scroll = next;
            self.dirty = true;
        }
    }

    pub(crate) fn cycle_theme(&mut self) {
        let names = Palette::NAMES;
        let current = names
            .iter()
            .position(|name| *name == self.theme)
            .unwrap_or(0);
        self.theme = names[(current + 1) % names.len()].to_string();
        self.rebuild();
        self.dirty = true;
        self.set_status(format!("主题：{}", self.theme), false);
    }
}
