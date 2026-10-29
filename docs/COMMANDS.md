# 命令与快捷键

## CLI

不带子命令时启动 TUI。

| 命令 | 说明 |
|---|---|
| `rhubtui list` | 打印一次设备快照（含拓扑、归属和功能节点） |
| `rhubtui mount [设备]` | 挂载分区；无分区时挂整盘 |
| `rhubtui umount [设备]` | 卸载分区或整盘 |
| `rhubtui eject [设备]` | 安全弹出（要求已卸载） |
| `rhubtui doctor` | 检查依赖与 D-Bus 服务 |
| `rhubtui config [--init]` | 显示配置；`--init` 写入默认配置，`--force` 覆盖 |

全局参数：

- `--json`：查询/操作子命令输出 JSON
- `-h, --help` / `-V, --version`

设备选择器可以是：

- 端口：`7-1.4`
- VID:PID：`5253:1020`（也接受 `0x5253:0x1020`）
- 块设备：`/dev/sdc1`、`sdc1`、`sdc`（挂载/卸载/弹出）

示例：

```bash
rhubtui list
rhubtui mount sdc1
rhubtui mount            # 第一个未挂载分区
rhubtui umount
rhubtui eject sdc
rhubtui doctor
rhubtui config --init
```

### `list` 输出

表格列：

| 列 | 含义 |
|---|---|
| `PORT` | sysfs 端口名，如 `7-1.4` |
| `LINK` | 物理连接：`根端口 2` / `Hub 7-1 · 4 号口` / `扩展坞 …` |
| `ID` | VID:PID |
| `TYPE` | `storage` / `input` / `audio` / `hub` / `other` |
| `PLACE` | `内置` / `外接`，接在扩展坞上时追加 `/坞上` |
| `NAME` | 产品名 |
| `EXTRA` | HID 节点、存储/挂载状态、声卡、摄像头等 |

`list --json` 每个设备包含：

- 拓扑：`sysfs`、`bus`、`devnum`、`port`、`root_port`、`depth`、`chain`、
  `parent`、`children`、`attachment`、`link`
- 归属：`place`（`internal` / `external`）、`place_reason`、`dock`、
  `hub_kind`（`hub` / `dock`）
- 控制器：`controller.slot` / `driver` / `vendor` / `device` / `buses`
- 设备：`id`、`name`、`category`、`speed`、`usb_version`、`bcd_device`、
  `manufacturer`、`product`、`serial`、`power_ma`、`authorized`、`ltm_capable`
- 功能：`interfaces`、`hids`、`disks`、`audio`、`videos`

**0.4.0 起的字段变化**：`sysfs` 表示端口名（`7-1.4`），`port` 表示该设备
接在父设备上的端口号（数字）。脚本里若把 `port` 当字符串用，需要改为 `sysfs`。

## TUI 按键

| 按键 | 动作 |
|---|---|
| `↑` `↓` / `k` `j` | 选择设备 |
| `Home` / `End` / `g` `G` | 跳到首/尾 |
| `Tab` / `Shift+Tab` | 跳到上/下一个分类 |
| `←` `→` / `h` `l` | 选择存储分区 |
| `m` / `u` | 挂载 / 卸载 |
| `e` | 安全弹出（弹确认框，`y`/Enter 确认） |
| `i` | 显示 / 隐藏内置设备 |
| `r` | 立即重新扫描 |
| `/` | 按名称/端口/VID:PID/卷标过滤 |
| `PgUp` / `PgDn` / `J` `K` / `Ctrl+D` `Ctrl+U` | 滚动详情 |
| `t` | 切换主题（本次运行） |
| `?` | 帮助（内容超高时可滚动） |
| `q` / `Esc` / `Ctrl+C` | 退出；有过滤时先清除过滤 |

过滤：

- 输入时实时筛选，`Enter` 确认并保留过滤，`Esc` 清空，`Ctrl+U` 清空
- 过滤条件同时匹配设备名、端口、VID:PID、序列号、磁盘/分区/卷标
- 过滤生效时标题显示 `/关键字`

鼠标：

- 滚轮在详情区滚动详情，在列表区上下移动选择
- 左键点击列表行选中设备
- 不想占用鼠标（需要终端里选中文本）时，在配置里设 `mouse = false`
