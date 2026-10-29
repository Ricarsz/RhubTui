# 配置与限制

## 配置文件

路径：`$XDG_CONFIG_HOME/rhubtui/config.toml`，回退到 `~/.config/rhubtui/config.toml`。
文件不存在时全部使用默认值，不会自动创建。

```toml
# 设备扫描间隔（毫秒），最小 200
scan_interval_ms = 1000

# 配色：ocean | forest | sunset
theme = "ocean"

# 滚轮滚动 / 左键选择；需要终端里选中文本时设为 false
mouse = true
```

写入默认配置：

```bash
rhubtui config --init          # 已存在时拒绝覆盖
rhubtui config --init --force  # 覆盖
rhubtui config                 # 显示路径与当前值
rhubtui --json config          # JSON，含可选主题列表
```

`t` 键只切换本次运行的主题；要改默认值请编辑配置文件。

## 刷新

- **设备扫描**：默认每 1 秒在后台线程扫描一次，不阻塞界面。
- **手动刷新**：`r` 立即重新扫描。
- **操作超时**：挂载/卸载/弹出有 30 秒超时，超时后状态行提示。

## 主题

- `ocean`：青蓝（默认）
- `forest`：绿色
- `sunset`：橙红

主题只影响界面框架、选中行和状态色；存储等语义色随主题调整。

## 依赖与权限

| 功能 | 依赖 |
|---|---|
| 设备枚举、详情 | 无（读 sysfs、`/proc`、`lsblk`） |
| 挂载/卸载/弹出 | `udisks2`（D-Bus 按需激活即可） |

`rhubtui doctor` 会逐项检查并给出修复提示。

## 物理归属与拓扑

每个设备都会计算：

- **内置 / 外接**
  - 根端口 `removable = fixed` → 内置
  - 根端口 `connect_type = hardwired` / `not used` → 内置
  - `removable = removable` / `connect_type = hotplug` → 外接
  - 两者都没有或为 `unknown` → 默认外接
  - 接在 Hub 后的设备继承上游根端口的判定，详情里注明「接在内置/外接 Hub 上」
- **根总线 / 根端口**：顶层 sysfs 名 `7-1.4` 表示总线 7、根端口 1；
  根端口直连时层级为 0，每经过一级 Hub 层级 +1
- **控制器**：从根 hub 的父目录读 PCI 地址、驱动和 vendor:device；
  同一控制器常同时提供 USB2/USB3 两条根总线，会合并显示
- **Hub 层级与端口**：显示从控制器 → 根端口 → 各级 Hub → 设备本身的链路
- **Hub / 扩展坞**（启发式）
  - 名称含 `dock` / `docking` / `扩展坞` / `thunderbolt` → 扩展坞
  - 下挂设备的功能接口里有 ≥2 类扩展坞常见功能（音频、CDC、图像、打印、
    网络、摄像头、Billboard）→ 扩展坞
  - 否则按普通 Hub 显示

扩展坞判定是启发式的，不是硬件真相：没有视频/网口/音频的 USB-C Hub
会显示为 Hub；名称或功能齐全的坞站会显示为扩展坞。详情页始终列出下挂设备、
USB 接口类别和判定依据，方便自行判断。

## 已知限制

- **热插拔**：1 秒轮询刷新，不是内核事件驱动。
- **端口信息**：部分内核没有暴露 USB port 设备，读不到 `connect_type`，
  此时内置判定主要依赖 `removable`；机箱前面板与主板后置口无法区分。
- **Type-C / 雷电**：不做 USB-C Alt Mode、Thunderbolt 拓扑映射；
  Billboard 设备只作为功能信息展示。
- **挂载权限**：非 root 依赖 udisks2 的 polkit 策略；个别 USB 硬盘盒会要求授权。
- **只读**：RhubTui 不打开 `hidraw`、不向任何设备写入；键鼠配置请用厂商驱动。
