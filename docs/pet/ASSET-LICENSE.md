# 鲸鱼娘立绘许可核实

核实时间：2026-11-23（UTC）。只查公开来源，没有调用任何生成接口，没有联系任何人，没有修改参考仓。本文不是法律意见。

本文替代 `docs/pet/DESIGN.md` §10.1 和 `docs/pet/ACCEPTANCE.md` 中「没有图片许可证据」的判断：上游所有者已公开声明立绘按 MIT 发布。

## 结论

| 层面 | 分级 |
| --- | --- |
| 一、仓库作者对立绘的许可 | **明确可用**：所有者公开书面声明 92 张立绘按 MIT 发布，允许再分发与演绎 |
| 二、生成服务与模型的输出权利 | **未能核实**：生成时使用的中转服务没有可查的条款，模型方条款只约束其直接客户 |
| 总体 | **有较强许可依据但有缺口**。缺口在生成链条，不在作者授权 |

接入前不需要用户另外提供授权证明：作者的公开声明本身就是授权证据（见下文 A2）。只要按 MIT 保留声明，并且接受生成链条这一残余风险，就可以接入。如果要商用，或希望消除残余风险，再按文末清单请作者确认。

## 一、仓库作者的许可证据

查过的版本：上游 `Sutera-Diffusus/dsh-whale-musume` @ `2d35234a7a01b18bdb3ad6c32e646b3d10e87828`（2026-11-22）；fork `Ocean-lhy/dsh-whale-musume` @ `6e77caed46da3629921b6a4b2b7dacb0838b36b6`。两者的 `assets/generated` 逐字节相同（`diff -rq` 无差异）。

**A1. LICENSE 与 package.json**
- 来源：<https://github.com/Sutera-Diffusus/dsh-whale-musume/blob/main/LICENSE>
- 内容：MIT，「Copyright (c) 2026 Sutera-Diffusus」，许可对象是 “this software and associated documentation files”。
- 历史：LICENSE 只有一次提交，即 `cef630457e36c9ccff3ede0cf00b921333213805`（2026-08-16，Initial private release v1.0.0）。同一提交加入了 46 张立绘，其余 46 张在 `68c7bca8fd4cf4dc6982153c51917608f19f2c93`（2026-08-18，v1.4.0）加入。两次提交的作者都是 Sutera-Diffusus，此后立绘没有改动。
- `package.json`：`"license": "MIT"`，`files` 包含 `assets/generated`，也就是说作者把立绘放进了 MIT 包的发布内容。
- 仓库里没有单独的图片许可，没有排除图片的条款，也没有第三方素材声明。
- 单看这些，只能说明立绘在 MIT 发布物里、没有被排除，还不足以说明作者有意用 MIT 覆盖图片。A2 补上了这个缺口。

**A2. 所有者在 issue #17 的明确表态（关键证据）**
- 来源：<https://github.com/Sutera-Diffusus/dsh-whale-musume/issues/17>，标题「【非官方移植】dsh-whale-musume 的 Rust 原生 Windows 桌面版」，由 ClutchBear 于 2026-09-22 提出。
  - 提问原文：「据我了解这些图是用第三方图像接口生成 + 人工确认后发布在你的 MIT 仓库里的。我按 MIT 授权理解……如果你的理解不同，告诉我」。
- 回复：<https://github.com/Sutera-Diffusus/dsh-whale-musume/issues/17#issuecomment-6167232963>，作者 Sutera-Diffusus，身份 OWNER，2026-10-14T11:24:32Z，未编辑。原文：
  > 本仓库（含 92 张立绘，以及由 `tools/gen-data.mjs` 从 `whale-moe-core.js` 导出的数据表）是按 **MIT** 发布的。MIT 允许再分发与演绎，前提是保留版权声明与许可原文 …… **这一项没有问题，不需要替换素材。**
  >
  > 沿用「鲸鱼娘」说明移植来源可以接受，前提是继续清楚标注**非官方、无隶属关系** …… 不要出现可能被误认为「官方 Windows 版」的表述或标识。
  >
  > 一个小请求：README 里保留指回上游的链接
- 再次确认：<https://github.com/Sutera-Diffusus/dsh-whale-musume/issues/17#issuecomment-6717663825>，2026-11-08T05:19:53Z，OWNER。原文：「代码与素材继续按 MIT，命名/非官方声明口径不变」。
- README「致谢」一节也引用了这件事：「MIT 归属已在 #17 确认」。
- 这条声明是在提问者明确指出「立绘由第三方接口生成」之后给出的。

**A3. 公开先例**
<https://github.com/ClutchBear/workbuddy-whale-musume> @ `b33d2e4a9112bae96c27590455815f60a9ffc6ec` 原样再分发了全部立绘，作者在 #17 认可了它的做法。它的归属方式是：
- `LICENSE` 写双段版权，含「Portions Copyright (c) 2026 Sutera-Diffusus」；
- `LICENSE-upstream` 保留上游许可原文；
- README 列出复用清单，并在标题、简介中标注非官方。

**A4. 其他公开渠道**
- 检索过上游全部 issue 和 PR，关键词包括立绘、许可、license、商用、授权、素材、版权、copyright。只有 #17 涉及许可，没有反对或限制用途的表态。
- 仓库的 3 个 discussion 都没有涉及许可。
- 「商用」一词在 issue 和 PR 中零结果。

**A5. 署名对象**
- 版权人是 Sutera-Diffusus（LICENSE 与 #17 回复者为同一账号）。
- README 致谢写「@SuteraWu —— 项目作者，立绘与表现层的主要实现」。两个账号是否同一人，没有公开说明。署名时以 LICENSE 的 Sutera-Diffusus 为准，致谢中可以一并提及 SuteraWu。

## 二、生成服务与模型的证据

**B1. 生成流程**
- `scripts/gen-assets.py` 在 `68c7bca`（v1.4.0）加入。它调用 `MODEL = "gpt-image-2"`，接口是 `{API}/images/edits`，以仓库自己的 `assets/generated/dsh-whale-state-idle-cute.webp` 作为底图做编辑。端点默认为 `https://free.quanthatch.com/c2a/v1`，可用 `DSH_IMAGE_API` 改，密钥取自 `DSH_JMRAI_API_KEY`。
- 海报脚本 `scripts/generate-promo-poster*.py` 使用 `https://jmrai.net/v1`。
- `CONTRIBUTING.md:31-35`：生成的立绘先经人工审阅，再进入 `assets/generated/`。
- 首批 46 张（`cef6304`）早于这个脚本，生成方式和底图 `idle-cute` 的来源都没有记录。

**B2. 中转服务条款**
- `https://free.quanthatch.com/` 返回 404；检索不到 QuantHatch 图像服务的公开条款，查到的 quanthatch.com 是一个虚拟交易平台。
- `https://jmrai.net/` 现在是 2026-10-10 的公告「JMRAI 并入 AGI Plan」。公告称其为「独立的聚合网关」，承诺「绝不声称与任何模型供应商存在官方授权或合作关系」（<https://agiplan.dev/promises>），但没有关于输出归属的条款。`agiplan.dev/terms` 返回 404。
- 2026 年 8 月生成时有效的条款版本查不到。

**B3. 模型方条款**
- OpenAI Terms of Use（<https://openai.com/policies/terms-of-use/>，Effective: January 1, 2026）原文：
  > As between you and OpenAI … you … own the Output. We hereby assign to you all our right, title, and interest, if any, in and to Output.
  >
  > output may not be unique … Our assignment above does not extend to other users’ output
- 这一条只约束 OpenAI 与它的直接客户。如果作者是经中转站调用的，中转站才是 OpenAI 的客户；中转站又把什么权利转给了作者，查不到。「gpt-image-2」是否确实由 OpenAI 提供，也无法从公开信息核实。

**B4. 其他未查项**
- AI 生成图像在各法域是否受著作权保护：本次不做判断。
- 角色与 DeepSeek 品牌（鲸鱼形象）相关的商标问题：本次未查，它与著作权许可是两回事。

## 三、按用途的已知条件与未知点

| 用途 | 作者层面（A） | 未知点（B） |
| --- | --- | --- |
| 原样使用 | MIT 允许，需保留声明 | 生成链条 |
| 缩放、裁剪、转码（webp→PPM/RGB565、像素化） | 「允许再分发与演绎」，需保留声明 | 同上 |
| 修改、衍生（重绘小图、拼帧） | 允许，需保留声明；不得暗示官方 | 同上 |
| 打包进本开源仓（GPL-3.0） | MIT 与 GPL-3.0 兼容；图片本身仍是 MIT，需随附上游 LICENSE 原文 | 同上 |
| 打包进二进制或安装包 | 发行物中需要包含版权与许可原文（例如随附 `LICENSE-upstream`，或提供许可显示入口） | 同上 |
| 未来商用 | MIT 原文包含 “sell”，作者声明未排除商用 | 同上，外加商标；商用前建议向作者确认 |

## 保留版权与署名的措施（接入时执行）

1. 新增 `LICENSE-upstream`（或 `third_party/dsh-whale-musume/LICENSE`），逐字保留上游 MIT 原文，含「Copyright (c) 2026 Sutera-Diffusus」。
2. README 与 `docs/pet/PACK.md` 写明：
   - 素材来源 <https://github.com/Sutera-Diffusus/dsh-whale-musume> 及取用的提交哈希；
   - 许可为 MIT，依据 #17；
   - 列出复用的图片和改动方式（缩放、裁剪、转码、像素化）。
3. 写明「非官方、与 dsh-whale-musume 作者无隶属关系」，不使用「官方」字样。
4. 二进制发行物随附许可原文。
5. 不清除上游已有的署名。

## 作者确认（可选，不是接入前提）

最小确认清单：

1. #17 的 MIT 口径是否同样适用于本项目（GPL-3.0 的 Cardputer Zero 终端程序），即缩小、像素化、转码后的立绘衍生图，内置在开源仓和设备二进制中。
2. 首批立绘及底图 `idle-cute` 是怎么生成的、用了哪个服务，作者是否知道该服务对输出权利有限制。
3. 「鲸鱼娘」名称与形象在商用场景下有没有额外要求。

给作者的中文草稿（只供用户审阅，未发送）：

> 你好，我在开发 Cardputer Zero 掌机上的 DeepSeek 终端程序 deepseek-harness-tui-cardputer（GPL-3.0，非官方，与你无隶属关系），想内置鲸鱼娘立绘，缩小、像素化成 320×170 小屏可用的帧。我看到你在 #17 说明立绘按 MIT 发布，会按 MIT 保留版权与许可原文，并注明非官方、附上游链接。想确认三点：
> 1. 这个口径是否也适用于缩放、像素化后的衍生图内置在 GPL 程序和设备二进制中？
> 2. 首批立绘和 idle-cute 底图所用的生成服务，你是否知道对输出再分发有限制？
> 3. 如果将来有商用分发，在名称或形象上有没有额外要求？
>
> 谢谢你的作品。
