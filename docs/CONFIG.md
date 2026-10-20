# 配置与限制

## 配置文件

路径：`$XDG_CONFIG_HOME/rhubtui/config.toml`，回退到 `~/.config/rhubtui/config.toml`。
文件不存在时全部使用默认值，不会自动创建。

```toml
# 设备扫描间隔（毫秒），最小 200
scan_interval_ms = 1000

# 电量自动刷新（秒），最小 10，只刷新读取成功的设备
battery_refresh_secs = 120

# 配色：ocean | forest | sunset
theme = "ocean"
```

`rhubtui config` 显示当前生效值与路径；`rhubtui --json config` 输出 JSON。

## 主题

- `ocean`：青蓝（默认）
- `forest`：绿色
- `sunset`：橙红

主题只影响界面框架、选中行和状态色；电量/挂载等语义色随主题调整。

## 依赖与权限

| 功能 | 依赖 |
|---|---|
| 设备枚举、详情 | 无（读 sysfs、`/proc`、`lsblk`） |
| 电量 | `/dev/hidraw*` 读写权限（udev `uaccess`） |
| 挂载/卸载/弹出 | `udisks2` |

`rhubtui doctor` 会逐项检查并给出修复提示。

## 内置与外接

- 每个设备会显示「内置」标记，`i` 可以隐藏/显示它们。
- 判定依据：设备在 sysfs 里的 `removable`（`fixed`）以及所在根端口的
  `connect_type`（`hardwired` / `not used`）。接在外部 Hub 后面的设备继承 Hub 的判定。
- 少数固件把端口标成 `unknown`，这类设备默认按外接处理。

## 已知限制

- **休眠**：2.4G 键鼠空闲后会休眠，鼠标可能不回应电量命令；移动一下再按 `b`。
- **热插拔**：1 秒轮询刷新，不是内核事件驱动。
- **挂载权限**：非 root 依赖 udisks2 的 polkit 策略；个别 USB 硬盘盒会要求授权。
- **型号差异**：键盘电量字段偏移来自 G75 V2，其他型号请先用 `probe` 核对。
- **盲发风险**：没有公开协议的设备只展示、不写入。
