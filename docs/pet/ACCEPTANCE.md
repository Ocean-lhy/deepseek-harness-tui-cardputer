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

# 实现后复核：`ca1b2b9`（Grok 4.7）

这一节是修复提交之后的复核，执行者是 Grok 4.7。它不是 Opus 独立验收，也不把实现报告里的自测当成证据。
上面 eb0ee76 的结论保持原样。

复核对象：`ca1b2b9fc638633268e58dfb0031c0e3eeb31610`（与 `origin/cursor/dsh-whale-pet-design-5cf2` 一致）。
环境：x86_64 云端 VM。没有连接 192.168.31.244，没有打开设备 framebuffer，没有安装服务，没有运行 `tools/deploy/deploy.sh`。
渲染用的是该提交的 debug 构建，资源包是 `/opt/cursor/artifacts/pet-pack-debug`，离屏输出是 `DST_PET_FB_OUT`，更新检查由 `DST_NO_UPDATE_CHECK=1` 关掉。模拟器是仓库里的 `tools/sim/screen_sim.py`。

## 结论

原 P0/P1 里，审批、菜单、提问、QR 文字层、artifact 会清掉宠物像素；命令输出不再被空闲房间盖住；API 失败和 Esc 取消不再记成成功；计数会落盘；导入要 `confirm`、会备份、会拒超限；`Task-master` 可以存；侧栏打开时落定正文按 43 列折行。这些有这次的代码阅读和运行记录。

整项仍不能写成通过。内置形象和字符兜底还不是鲸鱼娘。调试资源包的生产渲染可以认出蓝发女仆和鲸尾，侧栏缩图细节变粗，不能写成无损。保存的小程序占屏时像素层不让路。`usageStats` 的数字计数可以往返，字符串和小数不会原样留下。步数上限仍记成功。流式回复预览仍截最后 46 字。若干工具道具在深色侧栏上几乎看不见。本轮没有在设备上画过一帧。

可以进入用户已经授权的「临时目录 + timeout + 关闭自动更新」试运行，但不要按 `TRIAL.txt` 原文使用 dylan 的家目录。画面、闪烁、RGB565 字节序都还没在设备上验过。

## 逐项

| 项 | 结论 | 这次的证据 |
| --- | --- | --- |
| 审批清层 | 通过 | `sandbox_strict`，宠物开启。弹窗文字完整，侧栏没有鲸鱼娘。`/opt/cursor/artifacts/review-ca1b2b9/approval.png` |
| 菜单清层与关闭恢复 | 通过 | `/help` 全屏菜单没有宠物；Esc 后侧栏回来，记录里没有菜单标题残留。`help-menu.png`、`help-closed.png`。单测 `an_overlay_conceals_the_pet_and_the_next_frame_paints_it` 通过：菜单帧 PPM 全是品红键色，关闭后下一帧再画上 |
| 提问清层 | 通过 | `ask_user` 的 “Pick build mode:” 全屏，没有宠物。`ask-overlay.png` |
| QR | 部分 | 登录菜单和二维码说明文字层没有宠物，`login-menu.png`、`qr-guide.png`。本机没有 framebuffer，`draw_qr_fb` 失败后走文字说明，没有画出二维码点阵。点阵是否盖住宠物、关闭后是否擦掉，都还没在设备上看到 |
| artifact 清层与关闭 | 通过 | “Check panel” 三行全文可见，没有宠物；Esc 后回到对话并重新画出侧栏。`artifact.png`、`artifact-closed.png` |
| 小程序占屏 | 不通过 | `creator_apps_run` 的 QUICK APP 输出旁边仍画着鲸鱼娘。`app-run-pet-still-visible.png`。`apps::run_on_screen_args` 清的是终端，返回后只 `scr.draw()`，没有 `conceal()` |
| 命令输出不被房间挡住 | 通过 | 已看过提示的启动进空闲房间；`/pet stats` 的等级和心情留在画面上。`room-pack.png`、`command-visible.png`。未看过提示时先显示文字提示，不进房间，这是原来的引导，不是房间把输出盖住 |
| API 失败 | 通过 | 401 后画面是 `achievement: first-failure`，不是成功。`api-failure.png`。该场景随后的恢复回合才把 `successes` 记成 1，同时 `failures` 为 1 |
| Esc 取消 | 通过 | 取消帧只有 “cancelled”，没有成功成就。`cancelled.png`。存档里的 1 次成功来自后面那条 “are you there” |
| 步数上限 | 不通过 | `max_steps: 1` 打出 “step limit reached”，同时 `first-success`，存档 `successes: 1`。`run_turn` 只在 `report.cancelled` / `report.failed` 时改变命运，步数上限两个都不置位，于是落到 `TurnFate::Done` |
| 计数落盘 | 通过 | 摸头单测重载后 `pats == 1`，写失败时输出里有 “could not save”。工具回合结束后 `pet.json` 里有 `tools: 1`。写失败会留在 `last_save_error` |
| 导入 confirm / 备份 / 限额 | 通过，有剩余 | 单测：不带 `confirm` 不改存档；带上之后写入，并把旧档复制到 `pet.json.bak`（代码里权限 0600）。`reject_oversized(200_001)` 拒绝，`200_000` 放行。剩余：大小先看 metadata 再整文件读入，写入窗口里变大的文件仍会读进内存；备份只有一份，下次导入覆盖；路径按空白切分，带空格的路径对不上 |
| Task-master | 通过 | 单测导入标题 `Task-master` 并成功 `store_file`。旧的整文件 `sk-` 扫描已换成字段白名单 |
| usageStats | 部分 | 数字键可以往返：`successes` / `failures` / `pat` / `keywords`，以及未知数字键 `streak`。导出不再写旧名 `success` / `pats`。参考 `whale-moe-core.js` 的 `portableParse` 能保留这些数字，也能保留字符串 `note`。本仓 `usage_from` 只收数字，探针测试确认 `note` 被丢掉、`1.9` 被截成 `1`。这不是原样保真 |
| 43 列 | 通过，有剩余 | pty 报 49 列时，落定的 80 个 `Z` 分成 43 + 37。`wrap-43.png`。单测 `dock_text_wraps_at_43_even_when_the_pty_is_narrower` 通过。流式预览仍是 `tail_chars(&self.acc, 46)`，生成过程中最后 46 字会伸进侧栏 3 列；落定后的正文才是 43 |
| 资源包辨识度 | 部分通过 | 房间 3 倍生产渲染能看出长蓝发、女仆头饰、白围裙和鲸尾。`room-pack.png`。侧栏是 28×32 放大 2 倍，五官变粗，不能写成和原图一样清晰。没有用内置简化小人代替这组截图 |
| 内置形象 / 字符兜底 | 不通过 | 没有资源包或 `render: text` 时，空闲画面仍是 `art.rs` 的字符鲸，不是少女。`text-fallback.png`。`TEXT_IDLE` 仍是圆形鲸。像素模式在包缺失时回落 `pet_sprites.rs` 的简化蓝发图 |
| 工具状态 | 部分 | 真实 bash 回合在侧栏右侧画出一块绿色方块，身体姿势不变。`tool-shell.png`。同一条生产 `render` 画出的 10 个道具里，shell / write / plan / note / ask 看得见；read、clock、gear 是深色，压在深色底上几乎看不见；artifact 对应空道具。`tool-props.png`。状态条仍是粉、橙、蓝加 `L1`，没有文字 |
| 许可证归属 | 文件在，授权范围没有新证据 | 提交里有 `third_party/dsh-whale-musume/LICENSE` 和 `NOTICE`。资源包里也有。9 张上游 webp 的 SHA-256 与 `PROCESSING.json` 一致，来源修订 `2d35234a7a01b18bdb3ad6c32e646b3d10e87828`。NOTICE 写明非官方，且生成器条款没有找到。这次没有把 92 张原图放进 git |

## 清层实现里看到的边界

`Scr::draw` 在 approval、ask、menu、qr、artifact 任一存在时调用一次 `Host::conceal`：离屏 PPM 涂成品红键色，真面板则整屏填黑。之后这些层提前返回，不再 `present()`。QR 在 `qr_blit` 已为真时连文字层也不重画，避免把刚贴上的点阵涂黑。正常退出会再 `conceal()` 然后 `stty sane`。

没有信号处理器。`timeout` 发来的 SIGTERM 不会走到这段退出代码，终端可能停在 raw，面板上的最后一帧也不会被这段代码擦掉。

`/btw` 走 artifact 层，所以被上面的清层盖住。保存的小程序不走这条路径。

## 制品

本轮没有替换这些文件：

- `/opt/cursor/artifacts/dst-linux-aarch64`
- `/opt/cursor/artifacts/pet-pack-debug.tar.gz`
- `/opt/cursor/artifacts/TRIAL.txt`
- `/opt/cursor/artifacts/pet-pack-debug/`

aarch64 二进制这次重新量过：

- SHA-256 `af8c3e1108d824ed9d4b4636fe799e7b5f23690c0d658c426cecefb7200efc09`
- 1,777,344 字节
- ELF 64-bit LSB pie，ARM aarch64，解释器 `/lib/ld-linux-aarch64.so.1`，已 strip
- 动态符号最高到 `GLIBC_2.29`（`posix_spawn_file_actions_addchdir_np`）。其余是 2.17、2.18、2.25、2.27、2.28
- 用户量到的设备是 Debian 13.7、aarch64、glibc 2.41。需要的符号版本都低于 2.41，按符号看可以加载
- 本机没有 `qemu-aarch64`，这个二进制没有被执行。能加载不等于已经在设备上跑起来

资源包：

- tar SHA-256 `66ed9a093d520ecfb708085172b239d94601cf9e9ac63d2dc52588e63819e94e`
- 22 个成员，没有符号链接，没有 `..` 或绝对路径
- 目录模式 0777，文件 0666。解到多人可写的目录时，别人可以改这些文件
- 含 `idle.ppm`（P6，168×192，品红键色）以及另外 8 个姿态 PPM、LICENSE、NOTICE、PROCESSING.json，还有 9 张裁切后的 source PNG。不是 92 张原图
- 程序认 `DST_PET_PACK`，否则 `~/.dst/pet-pack`。目录不存在就用内置图。目录在但没有 `idle.ppm` 会报错
- 试运行要同时带上 `DST_PET_PACK=/tmp/dst-pet-pack`。只拷二进制不会出现这组鲸鱼娘

面板代码与用户量到的设备：

- `panel-mipi-dbid` 含 `mipi`，`name_is_panel` 接受。单测 `the_measured_panel_is_accepted_and_a_double_buffer_is_not` 通过
- `geometry_ok` 接受 16bpp、320×170、stride 640，拒绝虚拟高度 340
- 显式 `DST_FB=/dev/fb_lcd` 仍要过几何检查。`/dev/fb_lcd` 指向 fb0 时，按现有 sysfs 匹配可以找到这块屏
- 用户名 dylan。`config::home()` 在没有 `HOME` 时落到临时目录，不再写死 `/home/hanxiao`。hanxiao 只作为上游作者留在文档里
- RGB565 仍是小端，没有在设备上写过，不能据此说颜色正确

## 测试

- `cargo test --bin dst -- --test-threads=1`，跳过两条后台用例：151 过，1 败。失败的是 `checkpoint::tests::same_second_checkpoints_do_not_overwrite`（期望 2，得到 1）。单独再跑，这次提交和基线 `1059561` 都通过。与上一节记录的按秒取名的不稳定一致，不归因这次宠物修复。
- 这两条后台测试用 `setsid` 跑：`jobs_run_wait_and_kill`、`watcher_lines_are_queued`，2/2 通过。
- `node tools/sim/check_pet_migration.mjs` 对 `/tmp/ref-up/assets/whale-moe-core.js`：通过，WM1 与参考 `portableCode` 字节一致。该脚本不覆盖 `usageStats`。
- 另外两条只在临时工作区编译、没有提交的探针：非数字 `usageStats` 被丢弃；10 个道具帧写到 `/tmp/rv/props`。
- 宠物默认关闭、`DST_NO_UPDATE_CHECK=1`：`goal_manage` 和 `sandbox_full` 在本提交与基线 `1059561` 上以相同检查项失败（`only one round was spent`、`bottom-right badge shows the paused budget`、`round cap stops automatic continuation`；`the dangerous command really executed`）。不归因这次修复。`long_input` 这次在两边都通过。上一节记录它曾经失败，这次没有复现。

## TRIAL.txt

原文做到了这些：二进制和资源包只进 `/tmp`，不调用 `deploy.sh`，不安装到 `/usr/local/bin`，不起服务。`DST_NO_UPDATE_CHECK=1` 和 `DST_PET_TRIAL=1` 在 `update::enabled` 里都会关掉自更新。`DST_FB=/dev/fb_lcd` 指向用户量到的那块屏。

原文没有做到「不读秘密、不改用户文件」：

- 没有设置 `HOME`。进程用 dylan 的家目录，会读 `~/.dst` 里的配置和密钥文件。
- 一次启动就会写 `~/.dst/config.yml`、`history`、`sessions`、`skills/hardware-info`、`pet.json`。这次在模拟器的临时家目录里看到了这些文件。
- `pet.enabled` 默认仍是 false。按原文启动，除非 dylan 的配置里已经打开，否则看不到宠物。
- `timeout 120s` 的 SIGTERM 不会跑 `conceal()`，也不会 `stty sane`。

所以原文还不能直接拿去跑。建议主协调改用下面的临时家目录，跑完在同一个终端执行 `stty sane`。这只是验收记录，本轮没有改产品代码，也没有去连那台机器。

```
mkdir -p /tmp/dst-home/.dst
# 在这个新家目录放一份不含真实密钥的 config.yml，并写上 pet.enabled: true
chmod 755 /tmp/dst-trial
HOME=/tmp/dst-home timeout 120s env \
  DST_NO_UPDATE_CHECK=1 \
  DST_PET_TRIAL=1 \
  DST_PET_PACK=/tmp/dst-pet-pack \
  DST_FB=/dev/fb_lcd \
  /tmp/dst-trial
stty sane
```

不要在试运行里发需要真实 API 的对话，否则会按那份配置联网。本轮没有做这次设备绘制，不能把画面或闪烁写成通过。

## 仍须实机看的

- `/dev/fb_lcd` 实际打开、16bpp 小端颜色、stride 640 的每一行、有没有写到别的 framebuffer
- conceal 的整屏填黑和 fbterm 重绘会不会闪，关闭菜单后 43–52 列有没有残字
- 二维码点阵贴上之后，宠物有没有留在点阵下面；退出和 SIGTERM 之后面板和终端是否恢复
- aarch64 二进制在 Debian 13.7 上能否启动
- 侧栏 56×64 在真实 320×170 屏上是否还认得出
- 临时家目录试运行有没有碰到 `/usr/local/bin` 或 cpzterm。用户之前的查找不是全盘搜索，不能断言没有别的安装位置

## 截图

都在 `/opt/cursor/artifacts/review-ca1b2b9/`。来自上述生产路径，不是另画的图，也不是真机照片。

- `ask-overlay.png`、`qr-guide.png`、`login-menu.png`、`approval.png`、`artifact.png`
- `help-menu.png`、`help-closed.png`、`room-pack.png`、`command-visible.png`
- `wrap-43.png`、`api-failure.png`、`cancelled.png`
- `app-run-pet-still-visible.png`（未通过）
- `text-fallback.png`（字符鲸，未通过）
- `tool-shell.png`、`tool-props.png`、`contact.png`
