# 协议

2.4G 接收器在系统里只是普通 USB HID 设备，电量是厂商私有报文。
RhubTui 支持三类通道：MCHOSE 鼠标、MCHOSE 键盘、标准 HID Battery Strength。

## MCHOSE 鼠标（usage page `0xFF01`）

- 报告：命令 `0x11`（20 字节）、数据 `0x12`（64 字节）、推送 `0x13`（20 字节）
- 所有 payload 与 `0xFF` 异或，report ID 不参与
- 命令用 output report 下发，应答读 interrupt IN；Linux 下 feature report 是内核缓存回显
- 没有「读电量」命令：发 `0x0B 0xAA`（EEPROM 重载）后设备推 `0xE2`：

```
0xE2 状态包（异或解码后）
[3]     充电 0x01/0x00
[4]     电量 1..100
[6]     回报率索引 0..5 → 125/500/1000/2000/4000/8000 Hz
[9..17] 设备名 ASCII
```

睡眠时会推名字为空、回报率 0 的 `0xE2`，电量字节不可信，工具会重试。

## MCHOSE 键盘（usage page `0xFF31`）

64 字节帧，无 report ID：

```
[0]     0x55 请求 / 0xAA 应答
[1]     command
[2]     0
[3]     checksum = (size + offset_lo + offset_hi + data) & 0xFF
[4]     size（数据长度，最大 56）
[5..6]  offset LE16
[7]     0
[8..]   data
```

电量在 `0x05`（getFuncConfig）偏移 0 的 64 字节配置块里：

```
frame[40] & 0x7F  电量百分比
frame[40] >> 7    充电标志
```

请求固定为 `55 05 00 38 38 00 00 00` + 56 个 0（只读，不写配置）。

## 标准 HID 电量

report descriptor 里带 Generic Device Controls（page `0x06`）的 Battery Strength
（usage `0x20`）时，直接读 feature report，取第一个 ≤100 的字节。

## 多 HID 接口

同一接收器可能暴露多个 HID 接口（例如 G75 V2 有 6 个 hidraw 节点）。
RhubTui 逐个解析 report descriptor：

- 只有带 `0xFF01`、`0xFF31` 或标准电量 feature report 的接口会收到电量请求
- 其余接口只在详情/JSON 中展示，不发任何报文
- 电量结果按接口节点（`/dev/hidrawN`）缓存，互不覆盖

## 参考

- MCHOSE 鼠标协议逆向记录：<https://gitlab.com/unk9201422/Mchose/-/wikis/Protocol-Specification>
- REA（Reverse Engineer Anything）：<https://github.com/morluto/rea>
