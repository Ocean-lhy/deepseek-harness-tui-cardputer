# 第三阶段验收：鲸鱼娘桌宠（`eb0ee76`）

审查者：Opus 5.5 medium（独立验收，不沿用实现者报告）。审查对象：分支 `cursor/dsh-whale-pet-design-5cf2` @ `eb0ee762`，
基线 `main` @ `1059561`。环境：x86_64 Ubuntu 24.04 云端 VM，无 Cardputer Zero、无 `/dev/fb_lcd` / `/dev/fb_panel`。
本轮只验收，不改产品代码。

## 结论：不通过

主功能确实已接入并推送，WM1 核心字段与参考 JS 字节一致，53 列无宠物模式不回归。但有两项阻塞：
内置形象无法辨认为鲸鱼娘（用户第 1 项需求），以及菜单/审批等全屏层不清除宠物像素层（会遮挡审批文字）。
另有四项高优先级问题影响「失败状态」「双向迁移」「成就计数真实性」。

## 阻塞与问题（按严重度）

### P0 阻塞

**B1 形象不是可辨认的鲸鱼娘**（Grok 修复 / 用户决定素材）
- `src/pet_sprites.rs` `IDLE` 第 1–32 行：尖顶蓝色「兜帽」、粉色大嘴、蓝裙白块；所谓尾巴是第 25–31 行的 `t` 色块，渲染后是两条浅蓝裤腿。
  没有长蓝发、鲸鳍发饰/耳、女仆头饰、可见鲸尾（参考仓立绘的核心特征）。
- 证据：`docs/pet/review/room-girl-4x.png`（生产 `render`，房间 3× 再放大 4×）。60 px 侧栏中更像蓝帽小人。
- 单测 `the_girl_is_not_a_whale_silhouette` 只能证明「不是鲸鱼剪影」，不能证明「是鲸鱼娘」。
- 字符兜底（`docs/pet/preview/sim-text-room.png`）是圆形机器人脸，也不像少女。

**B2 全屏层不清除宠物层**（Grok 修复）
- `src/main.rs:677-688`：`draw()` 对 approval / ask / menu / qr 提前 `return`，到不了 `src/main.rs:1024-1026` 的 `present()`。
  宠物像素层保留上一帧。
- 复现：空闲房间下 `/help`，菜单被房间图盖住（`docs/pet/review/idle-room-over-help-menu.png`）；
  对话中 `/help`，侧栏盖住菜单右 10 列（`docs/pet/review/dock-over-help-menu.png`）。审批弹窗走同一路径，右侧命令文字会被遮挡，有安全含义。
- 关闭菜单后，第 0–8 行第 43–52 列仍留有菜单残字（`docs/pet/review/after-help-dock-residue.txt`），
  模拟器里被不透明侧栏盖住；真机 fbterm 任何重绘都可能露出。

### P1 高

**H1 像素空闲房间遮住命令输出**（Grok）
`src/main.rs:422-451` 只在空闲启动画面时进入房间，房间固定画在 0–9 行。在第一次对话前输入 `/pet …`、`/pet stats`、导入导出报错，
文字在 `.txt` 里有，但画面上被房间盖住。证据：`idle-room-hides-output.png` 与同名 `.txt`。

**H2 API 失败与 Esc 取消被记为成功**（Grok）
`src/harness.rs:796-808` 请求失败只发 `Ev::Note` 再发 `TurnEnd`；`src/pet.rs:207-218` 的 `TurnEnd` 在无工具错误时一律 `GEvent::Success`。
取消（`src/pet.rs:220-227`）也不置 `turn_error`。结果：失败或取消后显示开心姿态、心情 +3、计入 `first-success` / `success-10`。
复现：`docs/pet/review/review_probe.rs.txt` 中 `probe_api_failure_turn_is_celebrated`、`probe_cancel_turn_is_celebrated`（均按描述行为复现）。

**H3 `usageStats` 键名与参考仓不一致，双向迁移丢数据**（Grok）
本仓 `src/pet_growth.rs:418-422`、`545-552` 用 `success` / `failure` / `pats`；参考 `assets/dsh-whale-moe.js:1743-1753`、`860`
用 `successes` / `failures` / `pat`，另有 `keywords`。桌面存档导入后这些计数归零，再导出回桌面时丢失 `keywords`。
`tools/sim/check_pet_migration.mjs` 只校验核心字段，没覆盖这里。复现：`probe_reference_usage_stats_are_dropped`。

**H4 养成进度大多不落盘**（Grok）
`src/pet_ui.rs:123-130` 只在解锁成就时 `store_file`；另外只有启动签到（`46-51`）和 `/pet` 命令（`301-304`）会写。
工具/消息/成功计数、心情、饱食变化在不解锁时只在内存，退出即丢，`tools-50`、`messages-500` 等阈值实际难以达到。

### P2 中

- **M1 导入直接覆盖旧存档**：`src/pet_ui.rs:374-381` 无备份、无确认；`read_to_string` 在大小检查（`pet_growth.rs:440`）之前把整个文件读进内存。（Grok）
- **M2 凭据关键词误杀**：`src/pet_growth.rs:684-688` 对整个存档查 `sk-`，称呼/自称/日记含 `Task-`、`Whisk-` 即永远存不进去；
  调用方都是 `let _ =`，用户看不到。复现：`probe_title_with_sk_dash_blocks_every_save`。（Grok）
- **M3 默认关闭是未经确认的偏离**：设计稿 `a856971:docs/pet/DESIGN.md:211` 为 `enabled: true`，第 490 行建议「默认 `dock: auto`，可关」；
  用户确认的是「接受右侧宠物栏……允许关闭恢复」。现实现默认 `false`，并在 `docs/pet/DESIGN.md:11` 写成用户选择。（用户决定默认值；Grok 修正文档表述）
- **M4 工具道具区分度**：11 类工具渲染出 11 张不同帧（哈希各异），但身体姿势完全相同，只换右上 16×12 px 道具（侧栏 x 40–55、y 28–39）。
  read / web / schedule / generic 是深色描边图标，压在深色背景上几乎看不见；artifact 无道具。见 `docs/pet/review/tool-kinds-3x.png`（放大 3×）。
  另外 `contact.png` 的 think 帧带了计划板道具（`src/pet_ui.rs:415-423`），实际运行的思考姿态不画道具（`src/pet.rs:55-73`），这一格与真实运行不符。（Grok）
- **M5 空闲动画只有两帧**：`src/pet_art.rs:155-160` 每 16 tick（约 4.8 s）眨一次眼；实测 20 个 tick 只有 2 种画面。设计稿承诺的胸鳍、摆尾没有做。（Grok）
- **M6 状态条无标签**：粉/橙/蓝三条加 `L1`，无文字、无数值（`docs/pet/review/dock-bars-4x.png`）；好感条在低等级几乎是空的。（Grok）
- **M7 framebuffer 细节**：`src/fb.rs:83` 要求 `virtual_size` 恰为 320×170，双缓冲（yres_virtual > 170）的面板会被拒；
  不读 `pan`/`yoffset`；RGB565 固定小端（`src/fb.rs:158-160`），没有配置开关；每帧按 span 多次 `seek+write`，无与 fbterm 的同步。
  设备识别本身是保守的（几何 + 名称 + fbterm 祖先 / 显式路径），未发现会写错设备的路径。（字节序开关 Grok 可加；其余真机验证）
- **M8 MIT 声明**：`src/pet_growth.rs` 的 45 条成就名与参考 JS 逐字相同，文件头写「no file from that repository is copied」，但没有附 MIT 版权与许可声明。
  是否构成「substantial portions」不由本次判断；稳妥做法是附上声明。（用户决定；Grok 可补）

### P3 低

- `artifact` 在 `ToolCall` 时已减计数，`ToolResult` 又减一次（`src/pet.rs:177-189`），并行工具时可能提前结束工具姿态。
- `/pet export x.wm1` 时 JSON 与迁移码写到同一路径，后者覆盖前者（`src/pet_ui.rs:355-358`）。
- 侧栏打开时助手回复按 39 列折行，不是 43 列（`docs/pet/review/chat-43col-wrap.txt`）；关闭时按 52 列折行。不越界，只是少用 4 列。
- 文字兜底只有 idle / happy / sleep 三种画面。

## 需求完成矩阵

| 需求 | 结论 | 依据 |
| --- | --- | --- |
| 鲸鱼娘（非鲸鱼本体） | 不通过 | B1 |
| 像素为主、字符兜底 | 部分 | 两条路径都在；兜底不像少女；像素层不清除（B2） |
| 侧栏 43 列 / 关闭恢复 53 列 | 部分 | `/pet dock off/on` 实测 53/43；回复实为 39 列；全屏层遮挡（B2）；房间遮输出（H1） |
| 真实事件驱动姿态 | 部分 | 工具/思考/回复/压缩/完成接入；API 失败与取消显示为成功（H2） |
| 摸头/投喂/夸夸、三连 | 通过 | 数值与参考 `whale-moe-core.js:713-723` 一致 |
| 持久化养成 | 不通过 | H4、M2 |
| 29 个可触发成就 | 部分 | 29 个都有触发代码；成功计数被高估（H2），计数不落盘（H4） |
| WM1 / JSON 双向迁移 | 部分 | 核心字段与参考 `portableCode` 字节一致（本人重跑通过）；`usageStats` 丢数据（H3）；导入无备份（M1） |
| 隐私：密钥/聊天/工具参数 | 通过 | 存档无此类字段；两次模拟会话请求日志中无宠物内容 |
| 参考立绘不打包 | 通过 | 仓内无参考 webp；PR 产物不含参考图 |
| fb_lcd / fb_panel 安全探测 | 有条件 | 不会写未知设备；字节序/双缓冲/并发需真机（M7） |
| 零新增 crate | 通过 | `Cargo.toml` `[dependencies]` 为空 |
| ARM 可构建 | 有条件 | 见下方测试；未在设备上运行 |
| 小游戏后置 | 通过 | 未实现，成就 ID 保留不触发 |

## 测试结果（本人重跑）

- 分支 `cargo test --bin dst -- --test-threads=1 --skip jobs_run_wait_and_kill --skip watcher_lines_are_queued`：
  一次 142 过 1 败（`checkpoint::tests::same_second_checkpoints_do_not_overwrite`），另一次 148 过 0 败（含 5 条临时探针）。
- 干净基线 `1059561` 同命令：123 过 1 败，失败的也是 `same_second_checkpoints`。该用例在基线就会失败且时过时不过，属于既有的时间相关不稳定，不归因宠物改动。
- `bg::tests` 两条用 `setsid` 脱离会话后，基线与分支都 2/2 通过。实现者跳过的原因是测试进程组与代理 shell 冲突，不是代码问题；跳过可以接受，但应以脱离会话方式补跑。
- 新增单测 19 条，覆盖姿态、像素、WM1、默认关闭。未覆盖：全屏层与 `present`、`draw` 的 43 列输出、API 失败映射、`usageStats` 互操作、存档持久化时机、`fb::inspect` 假 sysfs。
- `node tools/sim/check_pet_migration.mjs <参考 core> target/pet-wm1-fixture.txt target/pet-json-fixture.json`：通过。参考仓不在 CI 中时该脚本直接跳过，CI 实际不做互操作验证。
- 全量场景（`DST_NO_UPDATE_CHECK=1`，debug 构建，宠物默认关）：102 过 5 败。失败的 `goal_manage`、`long_input`、`sandbox_full`、
  `update_command`、`update_offer` 在干净基线 `1059561` 上以完全相同的检查项失败，不归因宠物改动。
  其中两个 update 场景失败是因为全局设置了 `DST_NO_UPDATE_CHECK=1`，正好关掉了它们要测的更新路径。
- ARM：`aarch64-linux-gnu-gcc` 交叉链接 release 成功，`ELF 64-bit ARM aarch64`，1,773,128 字节；
  `qemu-aarch64 … --pet-preview sheet` 输出 13 帧，与 x86 逐字节相同。该构建依赖本 VM 的 `GLIBC_2.39`，设备 glibc 未知，需按仓库 CI 的 aarch64 产物或在设备上确认。
- PR 上没有任何 CI 检查记录。

## 未测项

真机 framebuffer 写入、闪烁与 fbterm 共存、RGB565 字节序、真机 FPS/CPU、审批弹窗下的实际遮挡、资源包真图效果（无授权素材）、全量场景在宠物开启时的表现。

## 截图与证据

全部来自生产渲染路径（debug 构建 `pet_art::render`，或 `DST_PET_FB_OUT` + `tools/sim/screen_sim.py` 叠加），不是真机。

- `docs/pet/review/room-girl-4x.png`：内置形象放大
- `docs/pet/review/tool-kinds-3x.png`：11 类工具侧栏帧
- `docs/pet/review/dock-bars-4x.png`：状态条
- `docs/pet/review/idle-room-over-help-menu.png`：房间盖住 `/help`
- `docs/pet/review/dock-over-help-menu.png`：侧栏盖住 `/help`
- `docs/pet/review/idle-room-hides-output.png` / `.txt`：房间盖住命令输出
- `docs/pet/review/after-help-dock-residue.txt`：菜单关闭后侧栏列残字
- `docs/pet/review/chat-43col-wrap.png` / `.txt`：侧栏打开时折行
- `docs/pet/review/review_probe.rs.txt`：复现 H2/H3/M2 与渲染工具帧的临时测试（未编入产品代码）

## 许可表述审查

`docs/pet/DESIGN.md` §10.1 与 PR 描述的措辞是谨慎的：没有说图片一定不获许可，也没有说已获授权，也没有凭「AI 生成」下结论。
`docs/pet/PACK.md:3`「代码的 MIT 文本没有单独授权那些图片」是事实陈述，但读者可能理解为「不在 MIT 范围内」，建议改为「是否覆盖图片不明确」。
内置像素图没有来源记录：建议在 `src/pet_sprites.rs` 或 DESIGN 写明由谁、以何种方式绘制（例如本仓实现代理逐像素编写、未使用外部图像生成），以便追溯。

## 分工

- 需要 Grok 修复：B1（重画或明确改为资源包优先）、B2、H1–H4、M1、M2、M4–M6、P3 各项、M3 的文档表述、M8 的声明（若用户同意）。
- 只需真机验证：M7 的设备节点选择、字节序、stride、双缓冲、fbterm 并发闪烁与残字、FPS；ARM 产物在设备 glibc 上运行。
- 必须用户决定：内置形象是否接受「原创简化像素」路线，还是向作者取得立绘授权后做资源包；宠物默认开启还是关闭；是否附 MIT 声明。
