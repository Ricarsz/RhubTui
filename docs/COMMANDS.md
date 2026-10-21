# 命令与快捷键

## CLI

不带子命令时启动 TUI。

| 命令 | 说明 |
|---|---|
| `rhubtui list` | 打印一次设备快照（不读电量，含拓扑、归属和功能节点） |
| `rhubtui battery [设备]` | 读取所有可读设备的电量 |
| `rhubtui mount [设备]` | 挂载分区；无分区时挂整盘 |
| `rhubtui umount [设备]` | 卸载分区或整盘 |
| `rhubtui eject [设备]` | 安全弹出（要求已卸载） |
| `rhubtui probe <节点>` | 抓取 HID 原始报文 |
| `rhubtui doctor` | 检查依赖、udev 规则、D-Bus 服务与 hidraw 权限 |
| `rhubtui config [--init]` | 显示配置；`--init` 写入默认配置，`--force` 覆盖 |

全局参数：

- `--json`：查询/操作子命令输出 JSON；`probe` 仍输出原始报文
- `-h, --help` / `-V, --version`

设备选择器可以是：

- 端口：`7-1.4`
- VID:PID：`5253:1020`（也接受 `0x5253:0x1020`）
- HID 节点：`/dev/hidraw13`（电量）
- 块设备：`/dev/sdc1`、`sdc1`、`sdc`（挂载/卸载/弹出）

示例：

```bash
rhubtui list
rhubtui battery
rhubtui --json battery
rhubtui battery 3837:3033
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
| `TYPE` | `storage` / `input` / `audio` / `network` / `hub` / `other` |
| `PLACE` | `内置` / `外接`，接在扩展坞上时追加 `/坞上` |
| `NAME` | 产品名 |
| `EXTRA` | 可读电量的 HID、存储/挂载状态、声卡、网卡、视频、串口、打印等 |

`list --json` 每个设备包含：

- 拓扑：`sysfs`、`bus`、`devnum`、`port`、`root_port`、`depth`、`chain`、
  `parent`、`children`、`attachment`、`link`
- 归属：`place`（`internal` / `external`）、`place_reason`、`dock`、
  `hub_kind`（`hub` / `dock`）
- 控制器：`controller.slot` / `driver` / `vendor` / `device` / `buses`
- 设备：`id`、`name`、`category`、`speed`、`usb_version`、`bcd_device`、
  `manufacturer`、`product`、`serial`、`power_ma`、`authorized`、`ltm_capable`
- 功能：`interfaces`、`hids`、`disks`、`audio`、`nets`、`videos`、`ttys`、`printers`

**0.4.0 起的字段变化**：`sysfs` 表示端口名（`7-1.4`），`port` 表示该设备
接在父设备上的端口号（数字）。旧版 `battery --json` 里的 `port` 字符串已改名为
`sysfs`，脚本需要同步调整。

### `battery` 的 JSON

```json
[
  {
    "device": "MCHOSE A7 Pro",
    "sysfs": "7-1.4",
    "node": "/dev/hidraw13",
    "iface": 2,
    "protocol": "MCHOSE 鼠标",
    "percent": 96,
    "charging": false,
    "polling_hz": 1000
  }
]
```

部分设备读不到时，成功的照常输出，失败信息写到 stderr；全部失败才返回非零。

## TUI 按键

| 按键 | 动作 |
|---|---|
| `↑` `↓` / `k` `j` | 选择设备 |
| `Home` / `End` / `g` `G` | 跳到首/尾 |
| `Tab` / `Shift+Tab` | 跳到上/下一个分类 |
| `←` `→` / `h` `l` | 选择存储分区 |
| `m` / `u` | 挂载 / 卸载 |
| `e` | 安全弹出（弹确认框，`y`/Enter 确认） |
| `b` | 重新读取电量 |
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

## 抓包

```bash
rhubtui probe /dev/hidraw2 --decode --seconds 15
```

`--decode` 会按 `0xFF` 异或解码 payload（MCHOSE 鼠标协议用），键盘协议不需要。
