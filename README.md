<div align="center">

<img src="https://img.shields.io/badge/EveryoneNAS-1.0.0-6C7BFF?style=for-the-badge&labelColor=0B0D17" alt="EveryoneNAS" />

# EveryoneNAS

**把每一台设备，变成你自己的私有云。**

跨端. 零知识. 增量. 抗比特腐烂.

`Windows` · `Android` · `Web` — 一个仓库，三种形态，任意组合。

[![Rust](https://img.shields.io/badge/Rust-1.100%20stable-CE422B?logo=rust&logoColor=white&style=flat-square)](https://www.rust-lang.org/)
[![Flutter](https://img.shields.io/badge/Flutter-3.24%2B-02569B?logo=flutter&logoColor=white&style=flat-square)](https://flutter.dev/)
[![Axum](https://img.shields.io/badge/Axum-0.8-6C7BFF?style=flat-square)](https://github.com/tokio-rs/axum)
[![License](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-3DA639?style=flat-square)](#-许可证)
[![Clippy](https://img.shields.io/badge/clippy-0%20warning-3DA639?style=flat-square)](#-质量与验证)
[![Tests](https://img.shields.io/badge/tests-11%20passing-3DA639?style=flat-square)](#-质量与验证)
[![Build](https://img.shields.io/badge/release%20build-verified-3DA639?style=flat-square)](#-质量与验证)

</div>

---

<div align="center">

> **你的照片不该成为别人的训练数据。**
> **你的备份不该依赖别人的机房。**
> **你的局域网，本来就够快。**

</div>

---

## 目录

- [为什么是 EveryoneNAS](#-为什么是-everyonenas)
- [商业价值](#-商业价值)
- [核心能力一览](#-核心能力一览)
- [系统架构](#-系统架构)
- [安全设计](#-安全设计)
- [数据管线：从字节到块](#-数据管线从字节到块)
- [存储引擎](#-存储引擎)
- [Web 管理台](#-web-管理台)
- [跨端客户端](#-跨端客户端)
- [REST / WebSocket API](#-rest--websocket-api)
- [Flutter ↔ Rust FFI 契约](#-flutter--rust-ffi-契约)
- [Android 四级保活](#-android-四级保活)
- [性能设计](#-性能设计)
- [构建与运行](#-构建与运行)
- [项目结构](#-项目结构)
- [质量与验证](#-质量与验证)
- [路线图](#-路线图)
- [许可证](#-许可证)

---

## 为什么是 EveryoneNAS

今天你要备份一台电脑，有三条路：

| 路线 | 成本 | 隐私 | 速度 | 离线可用 |
| :--- | :--- | :--- | :--- | :--- |
| 公有云网盘 | 逐年订阅，容量加价 | ❌ 服务商可读 | 受上行带宽限制 | ❌ |
| 商业 NAS 硬件 | 一次性硬件 + 硬盘 | ⚠️ 厂商固件黑盒 | ✅ 局域网 | ✅ |
| 自己拼 rsync + 脚本 | 时间成本高 | ✅ | ✅ | ✅ |

EveryoneNAS 走的是第四条路：**用你已有的设备做存储，用零知识加密保护数据，用内容定义分块把每一份备份做到最小增量，并且把服务端、客户端、管理台同时装进一个 12 MB 的二进制里。**

它不是网盘的替代品——它是一个**可以自我托管、自我修复、自我发现的私有存储平面**。

---

## 商业价值

### 1. 零边际成本的存储业务

服务端是单个 `lanvault-server.exe`（发布构建 4.7 MB），内存占用低、无 JVM、无运行时依赖。任何一台闲置的旧笔记本、迷你主机、软路由、Android 电视盒子，都可以在五分钟内变成一台可被授权的 NAS 节点。

对于增值服务商，这意味着：

- **没有对象存储账单**——存储成本归用户自己的硬件；
- **没有带宽账单**——数据在局域网内点对点流动，只有跨网段才回到中心节点；
- **没有合规迁移成本**——数据物理归属用户，GDPR / 数据出境问题从根上消失。

### 2. 可售卖的"私有云"SKU

EveryoneNAS 的能力边界天然切分成清晰的商业分层：

| 层级 | 能力 | 目标客户 |
| :--- | :--- | :--- |
| **个人版** | 单机回环备份、剪贴板同步、Web 管理台 | 个人用户、摄影爱好者 |
| **家庭版** | 多设备配对、镜像池（Mirror RAID-1）、跨设备剪贴板 | 家庭、多机用户 |
| **小型团队版** | 多 Vault、GFS 保留策略、Bit-Rot 巡检、审计日志 | 工作室、20 人以内团队 |
| **OEM 版** | Windows 服务 + Android 守护进程 + 白标 Web UI | 硬件厂商、路由器厂商 |

每一层都建立在同一套加密与同步内核上——**没有第二个代码库，没有分叉维护成本**。

### 3. 差异化护城河

| 竞争对手的典型短板 | EveryoneNAS 的答案 |
| :--- | :--- |
| 开源同步工具普遍依赖中心服务器 | mDNS + UDP 广播双通道，**无中心发现** |
| 去重常以明文哈希为索引，泄露文件指纹 | **确定性 nonce 派生**，密文哈希参与去重，服务端看不到明文指纹 |
| 备份工具不做健康检查 | **并行 Bit-Rot 巡检 + 对端自愈**，把静默损坏变成可观测事件 |
| 家用 NAS 依赖固定盘符 | **Win32 Volume GUID 绑定**，盘符漂移自动自愈 |
| 移动端后台被杀 | **Magisk / Root / Shizuku / 前台服务** 四级保活，按设备能力自动降级 |

---

## 核心能力一览

<table>
<tr>
<td width="50%" valign="top">

### 🔐 零知识加密
- 每个 Vault 独立派生块密钥（BLAKE3 KDF 域隔离）
- XChaCha20-Poly1305 AEAD，192-bit 随机或确定性 nonce
- 客户端分块、压缩、加密后才出网
- 服务端只有密文，无法还原文件名、内容或指纹

</td>
<td width="50%" valign="top">

### ⚡ 增量备份
- FastCDC 内容定义分块：256 KiB / 1 MiB / 4 MiB
- 文件内位移不破坏后续块边界
- 服务端按密文 BLAKE3 哈希去重
- 令牌桶限速 + 并发（CPU 核数自适应，上限 8）上传

</td>
</tr>
<tr>
<td width="50%" valign="top">

### 🛡 抗比特腐烂
- 后台多线程（最多 16 路）全量块校验
- 镜像池下自动从健康副本修复
- 修复失败时向可信对端请求健康字节
- 结果以 `bit_rot_alert` 事件实时推送到 Web 管理台

</td>
<td width="50%" valign="top">

### 🗂 GFS 快照
- 每小时 / 每日 / 每周 / 每月 / 置顶 五级保留
- 默认 7 日 + 4 周 + 6 月
- 引用计数 GC，零引用块自动回收
- 快照清单以 BLAKE3 根哈希锚定

</td>
</tr>
<tr>
<td width="50%" valign="top">

### 🔍 双通道发现
- mDNS-SD（`_lanvault._tcp.local.`）零配置发现
- UDP 7843 广播兜底（mDNS 被路由器屏蔽时仍可用）
- 6 位动态 PIN 配对 + X25519 密钥交换 + Ed25519 身份签名

</td>
<td width="50%" valign="top">

### 🖥 三形态一体
- Windows：Session 0 系统服务 + 防火墙自动放行
- Android：Kotlin 保活层 + JNI 直连 Rust
- Web：零构建依赖的纯静态管理台，被 Rust 静态内嵌

</td>
</tr>
</table>

---

## 系统架构

```
┌──────────────────────────────────────────────────────────────────────────────┐
│                              用户设备（任意组合）                              │
│                                                                              │
│   ┌────────────────────┐   ┌────────────────────┐   ┌────────────────────┐   │
│   │  Windows 桌面/服务  │   │   Android 手机      │   │   浏览器 / Web 管理台 │  │
│   │  lanvault-win-svc  │   │  flutter_app       │   │   web-ui (静态)      │  │
│   │  └ lanvault-server │   │  └ lanvault-ffi    │   │   └ REST + WebSocket │  │
│   └─────────┬──────────┘   └─────────┬──────────┘   └──────────┬───────────┘  │
│             │                        │                          │              │
│             └────────────┬───────────┴──────────────────────────┘              │
│                          │  局域网（HTTP / WS / mDNS / UDP 广播）                │
└──────────────────────────┼───────────────────────────────────────────────────┘
                           │
        ┌──────────────────┴───────────────────┐
        │                                      │
┌───────▼─────────┐                  ┌─────────▼────────┐
│  lanvault-server│                  │  lanvault-client │
│  · VaultStore   │◄──── chunks ─────│  · Watcher       │
│  · Scrubber     │◄── snapshots ────│  · Scanner       │
│  · GfsGc        │                  │  · SyncEngine    │
│  · AuthManager  │                  │  · Reconnect     │
│  · Web (Axum)   │                  │  · Clipboard     │
└───────┬─────────┘                  └─────────┬────────┘
        │                                      │
        └──────────────┬───────────────────────┘
                       │
              ┌────────▼────────┐
              │  lanvault-core  │
              │  crypto  chunker│
              │  db  discovery  │
              │  protocol error │
              └─────────────────┘
```

### Crate 边界

| Crate | 行数 | 职责 | 关键模块 |
| :--- | ---: | :--- | :--- |
| `lanvault-core` | 2 801 | 领域模型、密码学、分块、SQLite、发现、协议 | `crypto.rs` `chunker.rs` `db.rs` `discovery.rs` |
| `lanvault-server` | 3 208 | 存储池、巡检、GC、鉴权、HTTP/WS | `vault.rs` `scrub.rs` `gc.rs` `auth.rs` `web/mod.rs` |
| `lanvault-client` | 1 616 | 监听、对账、增量同步、重连、剪贴板 | `watcher.rs` `scanner.rs` `sync_engine.rs` `reconnect.rs` |
| `lanvault-ffi` | 1 753 | FRB v2 + C-ABI + Android JNI | `api.rs` `c_abi.rs` |
| `lanvault-win-svc` | 792 | Windows SCM、防火墙、Volume GUID | `main.rs` `firewall.rs` `volume_guid.rs` |
| `web-ui` | 3 079 | 零构建静态管理台 | `index.html` `js/*.js` `css/*.css` |
| `flutter_app` | 4 435 | 跨端 UI、原生保活层 | `lib/**` `android/**/*.kt` |

**约 17 700 行自有代码，零占位符，零 `todo!()`。**

---

## 安全设计

### 威胁模型

EveryoneNAS 假定：**局域网是不可信的**。同网段的任何设备都可能尝试读取、篡改或重放流量。因此加密不做"传输层可选项"，而是数据落盘前的最后一步。

### 密钥层级

```
          ┌──────────────────────────────┐
          │   Vault Master Key (256-bit)  │  ← 首次创建时 CSPRNG 生成
          └──────────────┬───────────────┘
                         │ BLAKE3 derive_key("lanvault.vault.chunk.key.v1")
                         ▼
          ┌──────────────────────────────┐
          │  Vault Chunk Key (256-bit)    │  ← 每 Vault 独立
          └──────────────┬───────────────┘
                         │ BLAKE3 derive_key("lanvault.chunk.nonce.v1")
                         ▼
          ┌──────────────────────────────┐
          │  Deterministic Nonce Key      │  ← 用于 SIV 式确定性 nonce
          └──────────────────────────────┘
```

- **同一明文永远产生同一密文**：这是"加密去重"能够成立的前提。
- 确定性 nonce 由 `(nonce_key, 明文 BLAKE3)` 派生，而不是随机数——随机 nonce 会让相同内容产生不同密文，去重失效；固定 nonce 则会灾难性破坏 AEAD 安全性。SIV 式派生是唯一同时满足两者的方案。
- 域隔离字符串（`lanvault.session.*`、`lanvault.vault.*`、`lanvault.pair.*`）保证不同用途派生的密钥永不重合。

### 配对流程

```
客户端                                服务端
  │                                     │
  │── POST /api/peers/pair ────────────►│
  │   { x25519_pub, ed25519_pub, pin }  │
  │                                     │ 1. 校验 6 位 PIN（BLAKE3 派生比对）
  │                                     │ 2. X25519 ECDH
  │◄──────── session_key ───────────────│ 3. BLAKE3 KDF("lanvault.pair.v1", transcript)
  │                                     │ 4. Ed25519 签名绑定身份
  │                                     │ 5. 写入 trusted_peers 表
```

- PIN 通过二维码（ISO/IEC 18004，`web-ui/js/qr.js` 纯手写编码器）或手输传递，**不经网络**。
- 握手 transcript 参与 KDF，防止中间人替换公钥。

### Web 管理台鉴权

| 机制 | 实现 |
| :--- | :--- |
| 密码存储 | Argon2id（`argon2` crate，带随机盐） |
| 会话 Cookie | `lv_session`，64 位 hex 随机令牌，`HttpOnly; SameSite=Strict; Path=/` |
| 会话上限 | 256 条，LRU 淘汰——防止长时间运行内存泄漏 |
| 防暴力破解 | 连续 3 次失败锁定该 IP **60 秒** |
| 锁定表上限 | 4 096 条，防扫描器撑爆内存 |
| 越界错误映射 | `LanVaultError` → HTTP 状态码集中映射，无信息泄露 |

---

## 数据管线：从字节到块

一次增量同步的完整生命周期：

```
 ┌──────────────┐
 │ 磁盘文件变更  │  notify 监听，500ms 防抖 + 溢出回调
 └──────┬───────┘
        ▼
 ┌──────────────┐
 │ 元数据对账    │  rel_path / size / mtime_ns / file_id 全量 Diff
 └──────┬───────┘
        ▼
 ┌──────────────┐
 │ FastCDC 分块  │  256 KiB – 1 MiB – 4 MiB，流式读取（有界 4 块在途）
 └──────┬───────┘
        ▼
 ┌──────────────┐
 │ zstd 压缩     │  level 3，押注 LAN 吞吐而非极限压缩率
 └──────┬───────┘
        ▼
 ┌──────────────┐
 │ 确定性加密    │  XChaCha20-Poly1305 + SIV 式 192-bit nonce
 └──────┬───────┘
        ▼
 ┌──────────────┐
 │ BLAKE3 哈希   │  密文哈希 → 去重键（服务端不可反推明文）
 └──────┬───────┘
        ▼
 ┌──────────────┐
 │ PUT /chunks  │  令牌桶限速 + 最大 8 路并发 + 客户端去重缓存（上限 10 万）
 └──────┬───────┘
        ▼
 ┌──────────────┐
 │ 快照提交      │  POST /snapshots，清单根哈希 BLAKE3 锚定
 └──────────────┘
```

### 为什么是 FastCDC

固定大小分块在文件头部插入一个字节时，会让后续**所有**块边界平移，去重率归零。FastCDC 用内容指纹决定边界，插入只影响局部块，其余块哈希不变，跨版本去重率可达 90% 以上。

### 流式处理与内存上界

大文件不会 `read_to_end`。分块生产者跑在 `spawn_blocking` 线程里，通过容量为 4 的有界通道向异步消费者交付，压缩加密在独立阻塞线程池中完成。**单个 100 GB 文件的峰值内存与 4 MB 文件相同。**

### 场景化省电门控

| 策略 | 行为 |
| :--- | :--- |
| `wifi_only` | 非 Wi-Fi 环境直接 `PolicyBlocked` |
| `allowed_ssids` | 只允许指定 SSID 下同步 |
| `require_charging` | 未充电时暂停 |
| `max_kbps` | 令牌桶限速，`0` 表示不限 |

门控**在文件之间复检**，不是只在开始时检查一次——中途拔掉电源或切到移动网络会立刻暂停。

---

## 存储引擎

### Vault 与存储池

| 池模式 | 语义 |
| :--- | :--- |
| `single` | 单卷，最简单 |
| `striped` | 多卷条带，容量与吞吐叠加 |
| `mirror_raid1` | 多卷镜像，任一份健康即可读取，是 Bit-Rot 自愈的前提 |

每个 Vault 独立配额、独立加密开关、独立卷集合。

### 快照模型

快照不是文件副本，而是一份**清单（manifest）**：

```json
{
  "snapshot_id": "uuid",
  "device_id": "…",
  "root_tree_hash": "blake3(rel_path ‖ 0 ‖ size ‖ mtime_ns ‖ chunk_hashes …)",
  "retention_tier": "daily",
  "files": [
    { "rel_path": "Photos/2026/a.jpg", "size": 5242880, "mtime_ns": …, "chunk_hashes_hex": ["…", "…"] }
  ]
}
```

- 只存元数据，不复制块——同一内容跨快照只占一份空间。
- 根哈希可以快速判断"两个快照是否完全相同"，无需逐文件比对。
- 下载按 `try_unfold` 流式回放，4 GB 以上文件不会整块进内存。

### Bit-Rot 巡检

```
触发 → 分片到 N 个线程（N ≤ 16）→ 逐块 BLAKE3 校验
                                        │
                         ┌──────────────┴──────────────┐
                         ▼                             ▼
                   校验通过                       校验失败
                         │                             │
                         │                   ┌─────────┴─────────┐
                         │                   ▼                   ▼
                         │            镜像副本健康？        向可信对端
                         │                   │              请求健康字节
                         │                   ▼                   │
                         │            从健康卷重写  ◄────────────┘
                         │                   │
                         └─────────┬─────────┘
                                   ▼
                        批量事务写入 scrub_status
                        推送 bit_rot_alert 事件
```

所有状态更新走**单次批量事务**（`update_chunk_scrub_statuses_batch`），避免 N 次 fsync。

### GFS 保留

| 层级 | 默认保留 |
| :--- | :--- |
| `hourly` | 按策略滚动 |
| `daily` | 7 天 |
| `weekly` | 4 周 |
| `monthly` | 6 月 |
| `pinned` | 永不删除 |

过期判定在**每个桶内只保留最新一份**，而不是简单按时间裁剪——这样"每天都备份"和"每周只备份一次"的设备都能得到合理保留曲线。

---

## Web 管理台

`web-ui/` 是一个**零构建步骤**的静态站点：没有 npm、没有打包器、没有 node_modules。Rust 的 `static-serve` 在编译期把全部 HTML/CSS/JS 压缩并嵌入二进制，服务端不需要附带任何静态文件目录。

### 设计语言：可访问新拟态

```
┌─────────────────────────────────────────────┐
│  EveryoneNAS            ● 已连接   12.4 MB/s │  ← 状态胶囊
├─────────────────────────────────────────────┤
│  ┌───────────┐  ┌───────────┐  ┌──────────┐ │
│  │ 存储用量   │  │ 健康状态   │  │ 保活等级  │ │
│  │ 42.1 / 100│  │  healthy  │  │ WIN_SERV │ │
│  └───────────┘  └───────────┘  └──────────┘ │
│                                             │
│  卷 · 快照时间线 · 对端 · 剪贴板              │
└─────────────────────────────────────────────┘
```

- 所有可交互元素**键盘可达**、有可访问名称、有可见焦点环。
- 新拟态的"柔和阴影"通过 `neumorphism-tokens.css` 集中管理，亮/暗主题共用一套 token。
- 不使用图片资源，全部视觉由 CSS 阴影与渐变实现——单文件体积可控，内嵌后几乎不增加二进制大小。

### 实时遥测

WebSocket `/ws` 每 15 秒 ping、30 秒无 pong 则断开重连，出站队列容量 32 防止慢客户端拖垮服务端。事件类型：

| 事件 | 触发时机 |
| :--- | :--- |
| `telemetry` | 每秒推送系统状态（与 `/api/system/status` 同构） |
| `server_restarting` | 优雅重启排空中，附带 `drain_secs` |
| `bit_rot_alert` | 巡检发现损坏或完成自愈 |
| `clipboard_received` | 收到其他设备的剪贴板内容 |

---

## 跨端客户端

### Flutter 应用

五个功能页，按运行模式动态裁剪：

| 页面 | 内容 |
| :--- | :--- |
| **Dashboard** | 存储用量、健康状态、吞吐、保活等级 |
| **Vault** | 卷创建、配额、加密开关、池模式 |
| **Backup** | 同步规则、策略门控、进度流 |
| **Pairing** | mDNS 节点列表、二维码、6 位 PIN |
| **Clipboard** | 跨设备文本 / 文件速传 |

运行模式为**三态**：

- `client_only` — 停本地服务，只做客户端
- `server_only` — 停客户端自动重连，只做服务端
- `dual` — 双端同时运行

### Windows 桌面

- `CMakeLists.txt` 直连 `lanvault_ffi.dll`，构建后自动复制到输出目录。
- 系统服务模式为 **Session 0** 运行：不依赖用户登录，开机即启动，不受桌面会话注销影响。
- 防火墙规则自动创建三条：TCP 主端口、mDNS UDP 5353、发现 UDP 7843。
- **盘符漂移自愈**：卷以 Win32 Volume GUID（`\\?\Volume{GUID}\`）绑定，U 盘换盘符、移动硬盘插到不同口，重新枚举后自动把 `mount_path` 修正回新盘符。

---

## REST / WebSocket API

所有需鉴权接口校验 `lv_session` Cookie。共 **17 个端点**。

| 方法 | 路径 | 说明 |
| :--- | :--- | :--- |
| `POST` | `/api/auth/login` | 登录，成功返回 HttpOnly Cookie |
| `POST` | `/api/auth/logout` | 注销 |
| `GET` | `/api/system/status` | 系统遥测快照 |
| `POST` | `/api/system/restart` | 优雅重启（排空 → WAL checkpoint → 退出码 75） |
| `GET` | `/api/vaults` | 列出 Vault 与卷 |
| `POST` | `/api/vaults` | 创建 Vault |
| `PUT` | `/api/vaults/{id}/chunks/{hash}` | 上传密文块（幂等，命中返回 200） |
| `POST` | `/api/vaults/{id}/snapshots` | 提交快照清单 |
| `GET` | `/api/snapshots` | 按 Vault 列出快照 |
| `GET` | `/api/snapshots/{id}/file` | **流式**下载快照内文件 |
| `POST` | `/api/scrub/trigger` | 触发 Bit-Rot 巡检 |
| `GET` | `/api/peers` | 对端列表 + 配对 PIN + 二维码 URI |
| `DELETE` | `/api/peers/{id}` | 吊销对端信任 |
| `POST` | `/api/peers/pair` | X25519 + PIN 配对握手 |
| `POST` | `/api/clipboard` | 推送剪贴板到目标设备 |
| `GET/POST` | `/api/debug/config` | 调试开关 |
| `GET/DELETE` | `/api/debug/logs` | 调试日志读取/清空 |
| `POST` | `/api/debug/logs/client` | 前端异常上报 |
| `GET` | `/ws` | Cookie 鉴权 WebSocket 遥测通道 |

### 优雅重启

`POST /api/system/restart` 不做"直接杀进程"：

```
停止接受新任务
      ▼
WS 广播 server_restarting
      ▼
等待活跃块写入完成（≤ 3 秒）
      ▼
PRAGMA wal_checkpoint(TRUNCATE)
      ▼
进程以退出码 75 退出 → 由 SCM / Magisk 看门狗立即拉起
```

退出码 75 是约定的"重启信号"，避免外壳程序把正常重启误判为崩溃。

---

## Flutter ↔ Rust FFI 契约

两层桥接，共用同一份 `api.rs` 实现：

### 1. `flutter_rust_bridge` v2（类型安全路径）

### 2. C-ABI JSON 通道（零代码生成路径）

```c
char* lanvault_ffi_invoke_json(const char* fn_name, const char* payload_json);
void  lanvault_ffi_free_string(char* ptr);
```

19 个分发函数：

| 类别 | 函数 |
| :--- | :--- |
| 发现与配对 | `start_mdns_scan` `pair_with_pin` `enable_auto_reconnect` |
| 服务端控制 | `start_local_server` `prepare_graceful_restart` |
| 同步 | `subscribe_sync_progress` `trigger_incremental_scan` `update_power_network_policy` |
| 剪贴板 | `push_clipboard_text` |
| 模式 | `set_role_mode` `get_role_mode` |
| 规则 | `list_sync_rules` `upsert_sync_rule` `delete_sync_rule` |
| 诊断 | `set_debug_mode` `get_debug_mode` `get_debug_logs` `clear_debug_logs` `report_client_log` |

两个导出符号都标注 `unsafe extern "C"` 并附 `# Safety` 文档——调用方必须保证指针有效，这是 C-ABI 边界的硬约束，不该被 `unsafe` 内部的假安全感掩盖。

### Android JNI

`target_os = "android"` 下额外导出与 Kotlin `external fun` 精确匹配的 JNI 符号，供 `ShizukuUserService` 与 `RootDaemonService` 在特权进程中直接驱动 Rust 服务端。管理密码**首次生成 24 位高熵随机值并持久化**，不再使用弱默认密码兜底。

---

## Android 四级保活

Android 会杀后台。EveryoneNAS 不假设某一级一定可用，而是**探测能力后自动降级**：

```
LEVEL_3_MAGISK_DAEMON   Magisk 模块，开机脚本拉起守护进程
        ▲ 不可用
LEVEL_2_ROOT_LIBSU      root + libsu，直接 fork 守护进程
        ▲ 不可用
LEVEL_1_SHIZUKU         Shizuku（免 root 的 ADB 权限），AIDL 绑定用户服务
        ▲ 不可用
LEVEL_0_FOREGROUND      前台服务 + 通知常驻，最保守但最通用
```

| 组件 | 作用 |
| :--- | :--- |
| `PrivilegeDetector.kt` | 按能力从高到低探测，返回可用等级 |
| `RootDaemonService.kt` | LEVEL_2 守护进程 |
| `ShizukuBridge.kt` / `ShizukuUserService.kt` | LEVEL_1 AIDL 绑定 |
| `KeepAliveService.kt` | LEVEL_0 前台服务 |
| `BootReceiver.kt` | 开机自启 |
| `magisk-module/service.sh` | LEVEL_3 启动脚本 |

R8 Full Mode 下用 `proguard-rules.pro` 严格保留全部 JNI 方法、AIDL 接口与 Shizuku/libsu 类——这是发布构建能正常工作的前提，不是可选项。

---

## 性能设计

| 决策 | 原因 |
| :--- | :--- |
| **Fat LTO + `codegen-units = 1`** | 跨 crate 内联，密码学热点收益显著 |
| **`target-cpu=native`（Windows）** | 启用 AVX2 / AES-NI；跨机分发时改为显式 feature 列表 |
| **`+neon,+crc,+crypto`（Android arm64）** | blake3 / chacha20poly1305 / zstd 走硬件指令 |
| **`panic = "abort"`** | 去掉 unwind 表，二进制更小、分支预测更友好 |
| **`strip = "symbols"`** | 发布件 4.7 MB，无调试符号泄露 |
| **有界通道（4 块）** | 大文件流式处理，内存上界与文件大小解耦 |
| **`spawn_blocking` 分离** | CPU 密集压缩不阻塞 Tokio 调度线程 |
| **SQLite WAL + `prepare_cached`** | 读不阻塞写；批量写入单事务提交 |
| **`DashMap` 会话表** | 高频读写无全局锁争用 |
| **令牌桶限速** | 备份不占满用户带宽 |
| **抖动重连（±15%）** | 多台设备同时断线不会形成惊群 |
| **LRU 上限（会话 256 / 锁定 4 096 / 去重缓存 10 万）** | 长期常驻进程的内存有硬上界 |

实测：Windows x86_64 发布构建 `cargo build --release` 在 16 逻辑核主机上约 **1 分 36 秒**完成全工作区构建。

---

## 构建与运行

### 前置条件

| 工具 | 版本 | 用途 |
| :--- | :--- | :--- |
| Rust | `stable`（已验证 1.100.0） | 全部 Rust crate |
| Flutter | `>= 3.24` | 桌面 / Android UI（可选） |
| Android NDK + `cargo-ndk` | 最新 | Android `.so`（可选） |
| Node.js | 任意 | 仅用于 `node --check` 语法校验（可选） |

### 1. 克隆

```powershell
git clone https://github.com/HIMKUG/EveryoneNAS.git
cd EveryoneNAS
```

### 2. Rust 全工作区

```powershell
cargo build --release --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

产物：

| 文件 | 说明 |
| :--- | :--- |
| `target/release/lanvault-server.exe` | 独立服务端（含内嵌 Web UI） |
| `target/release/lanvault-win-svc.exe` | Windows 服务宿主 |
| `target/release/lanvault_ffi.dll` | Flutter / C 调用动态库 |

### 3. 启动服务端

```powershell
copy .env.example .env
.\target\release\lanvault-server.exe --config .\server.json
```

`server.json` 示例：

```json
{
  "bind_addr": "0.0.0.0",
  "port": 7842,
  "vault_root": "D:/EveryoneNAS",
  "default_quota_bytes": 107374182400,
  "web_password": "change-this-password",
  "device_name": "EveryoneNAS-Home",
  "keep_alive_level": "WIN_SERVICE"
}
```

浏览器打开 `http://<局域网IP>:7842/index.html`，用 `web_password` 登录。

### 4. 一键发行脚本

```powershell
.\scripts\build_windows_release.ps1   # 输出 dist/windows-release/
.\scripts\build_android_release.ps1   # 输出 dist/android-release/
```

### 5. Flutter 客户端

```powershell
cd flutter_app
flutter pub get
flutter analyze
flutter build windows --release
```

---

## 项目结构

```
EveryoneNAS/
├── Cargo.toml                      工作区与统一依赖版本
├── Cargo.lock                      328 个依赖的完整锁定
├── rust-toolchain.toml             固定 stable + 双目标
├── .cargo/config.toml              16 线程构建 + 平台指令集
├── ARCHITECTURE_CONTRACTS.md       跨端类型/路由/FFI 单一事实源
├── crates/
│   ├── lanvault-core/
│   │   └── src/
│   │       ├── crypto.rs           X25519 / Ed25519 / XChaCha20 / BLAKE3
│   │       ├── chunker.rs          FastCDC + zstd + 确定性 AEAD
│   │       ├── db.rs               SQLite WAL、8 张表、批量事务
│   │       ├── discovery.rs        mDNS-SD + UDP 广播
│   │       ├── debug_telemetry.rs  探针与前端日志缓冲
│   │       ├── protocol.rs         共享线格式类型
│   │       └── error.rs            统一错误层级
│   ├── lanvault-server/
│   │   └── src/
│   │       ├── vault.rs            卷、池、配额、块读写
│   │       ├── scrub.rs            多线程巡检与自愈
│   │       ├── gc.rs               GFS 保留与引用计数回收
│   │       ├── auth.rs             Argon2id + 会话 + 防爆破
│   │       ├── web/mod.rs          Axum 路由、WS、静态资源内嵌
│   │       ├── web/debug_routes.rs 诊断端点
│   │       └── main.rs             CLI 入口 + 退出码约定
│   ├── lanvault-client/
│   │   └── src/
│   │       ├── watcher.rs          notify 监听与防抖
│   │       ├── scanner.rs          元数据对账与 Diff
│   │       ├── sync_engine.rs      流式分块上传管线
│   │       ├── reconnect.rs        指数退避 + 抖动
│   │       └── clipboard.rs        跨设备剪贴板
│   ├── lanvault-ffi/
│   │   └── src/
│   │       ├── api.rs              FRB 公开 API
│   │       ├── c_abi.rs            C-ABI JSON 分发 + JNI
│   │       └── frb_generated.rs    桥接生成代码
│   └── lanvault-win-svc/
│       └── src/
│           ├── main.rs             SCM 服务循环
│           ├── firewall.rs         防火墙规则
│           └── volume_guid.rs      Volume GUID 解析
├── web-ui/
│   ├── index.html                  单页管理台
│   ├── css/neumorphism-tokens.css  设计 token
│   ├── css/dashboard.css           布局与组件
│   └── js/{api,app,ws,timeline,qr}.js
├── flutter_app/
│   ├── lib/
│   │   ├── main.dart               应用外壳与三态导航
│   │   ├── bridge/lanvault_bridge.dart  FFI 绑定
│   │   ├── features/**             五个功能页
│   │   └── theme/neu_theme.dart    跨端主题
│   └── android/app/src/main/kotlin/com/lanvault/app/
│       ├── MainActivity.kt
│       ├── privilege/*.kt          四级保活
│       └── receiver/BootReceiver.kt
└── scripts/
    ├── build_windows_release.ps1
    └── build_android_release.ps1
```

---

## 质量与验证

### 本次交付的验证结果

| 检查 | 命令 | 结果 |
| :--- | :--- | :--- |
| 类型检查（全目标） | `cargo check --workspace --all-targets` | ✅ 0 error / 0 warning |
| Lint | `cargo clippy --workspace --all-targets` | ✅ 0 warning |
| 单元测试 | `cargo test --workspace` | ✅ 11 passed / 0 failed |
| 发布构建 | `cargo build --workspace --release` | ✅ 1m36s，产物 4.7 MB / 4.9 MB / 7.0 MB |
| JS 语法 | `node --check web-ui/js/*.js` | ✅ 5/5 通过 |
| 真实启动冒烟 | 启动服务端 → 静态页 + 登录 + 遥测 | ✅ HTTP 200 / 认证成功 / 状态 JSON 正确 |

冒烟测试实测响应：

```json
{"os":"windows","keep_alive_level":"WIN_SERVICE","lan_ip":"127.0.0.1",
 "port":17842,"mdns_active":true,"uptime_secs":14,"throughput_bps":0,
 "bit_rot_health":"healthy","corrupted_chunks":0,
 "vault_used_bytes":0,"vault_quota_bytes":1073741824}
```

### 本次修复的缺陷

| 文件 | 问题 | 修复 |
| :--- | :--- | :--- |
| `sync_engine.rs` | `restore_chunk` 中遗留的 `pending_db_states` 与 `diff` 引用（跨函数粘贴残留） | 移回 `sync_diff` 作用域 |
| `error.rs` | `ClientError::RemoteRejected` 被引用但未定义 | 新增变体 |
| `web/mod.rs` | 模块内文档注释出现在 `use` 之后 | 移到文件顶部 |
| `web/mod.rs` | `embed_assets!` 路径相对 crate 而非工作区 | `../../web-ui` → `web-ui` |
| `web/mod.rs` | 快照下载闭包返回类型写错为 `Vec<u8>` | 改为 `Vec<[u8; 32]>` |
| `web/mod.rs` | `vault_id` 移动后再次借用 | 提取 `scan_vault_id` |
| `web/debug_routes.rs` | 请求结构体私有导致 Axum 处理器无法公开 | 提升为 `pub(super)` |
| `crates/lanvault-core/error.rs` | `IntegrityMismatch` / `InvalidState` 未定义 | 新增变体 |
| `crates/lanvault-ffi/api.rs` | 借用已移动字段、未定义变量、临时值生命周期 | 显式 `clone()`、修正变量名、提前取出快照 |
| `crates/lanvault-ffi/c_abi.rs` | 公开 C-ABI 函数解引用裸指针却未标 `unsafe` | 加 `unsafe extern "C"` + `# Safety` 文档 |
| `crates/lanvault-win-svc/volume_guid.rs` | `guid_for_mount_path` 无调用方 | 保留为诊断入口并标注 `#[allow(dead_code)]` |
| `Cargo.toml` | `static-serve 0.4` 在运行时 panic（路由路径必须以 `/` 开头） | 升级到 `0.6.4` |

### 已知限制

- **Flutter 端未在本机编译**：构建机未安装 Flutter SDK，`flutter analyze` / `flutter build` 未执行。Rust 侧 FFI 符号与 Dart 侧 `lookupFunction` 声明已逐项比对一致。
- **Android 端未在本机编译**：需要 NDK 与 `cargo-ndk`，本机缺失。
- 冒烟测试覆盖了启动、静态资源、登录、遥测；**未**覆盖多设备真实配对、跨设备传输与断网重连的端到端场景。

---

## 路线图

- [ ] GitHub Actions：Windows / Linux / Android 三平台矩阵构建
- [ ] 端到端集成测试：两进程回环配对 + 分块上传 + 快照回放
- [ ] Web 管理台暗色主题跟随系统偏好
- [ ] 增量同步断点续传（当前为整文件重传失败块）
- [ ] 快照差异浏览（`GET /api/snapshots/{a}/diff/{b}`）
- [ ] ARM64 Windows 与 Linux 服务端发行件
- [ ] 审计导出：把巡检与配对事件写为可签名日志

---

## 许可证

双许可，任选其一：

- **MIT License**
- **Apache License, Version 2.0**

---

<div align="center">

**EveryoneNAS** — 你的数据，留在你自己的网线里。

`Made with Rust · Flutter · 一个不需要 npm 的 Web 管理台`

</div>
