//! 按键帮助的唯一数据源；TUI 只负责上色和排版。

#[derive(Debug, Clone, Copy)]
pub enum HelpLine {
    Heading(&'static str),
    Row(&'static str),
    Note(&'static str),
    Blank,
}

pub const LINES: &[HelpLine] = &[
    HelpLine::Heading("按键"),
    HelpLine::Row("  ↑ ↓ / k j       选择设备"),
    HelpLine::Row("  Tab / Shift+Tab 跳到上/下一分类"),
    HelpLine::Row("  ← → / h l       选择存储分区"),
    HelpLine::Row("  m / u           挂载 / 卸载选中分区"),
    HelpLine::Row("  e               安全弹出（弹确认框）"),
    HelpLine::Row("  PgUp/PgDn J/K     滚动详情"),
    HelpLine::Row("  Ctrl+D / Ctrl+U   滚动详情"),
    HelpLine::Row("  /               按名称/端口/VID:PID 过滤"),
    HelpLine::Row("  b               重新读取电量"),
    HelpLine::Row("  a               切换概览 / 高级详情"),
    HelpLine::Row("  r               立即重新扫描设备"),
    HelpLine::Row("  i               显示 / 隐藏内置设备"),
    HelpLine::Row("  t               切换主题"),
    HelpLine::Row("  ?               显示 / 隐藏本帮助"),
    HelpLine::Row("  q / Esc / Ctrl+C  退出（有过滤时先清除）"),
    HelpLine::Blank,
    HelpLine::Note("鼠标：滚轮滚动详情或列表，左键点击选择设备。"),
    HelpLine::Note("电量仅对 MCHOSE 接收器和标准 HID 电量通道可用。"),
    HelpLine::Blank,
    HelpLine::Note("  ? / Esc 关闭"),
];
