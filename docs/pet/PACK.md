# 鲸鱼娘立绘包

仓库里仍留着一套原创 28×32 像素女孩数据（随 `dst` 的 GPL-3.0 分发），但它不再作为画面。没有资源包时用字符鲸鱼娘，不把这套简化蓝发图画出来。

上游 92 张原立绘的许可见 `docs/pet/ASSET-LICENSE.md`：所有者公开声明按 MIT 发布（Copyright (c) 2026 Sutera-Diffusus），可以再分发和做衍生，必须保留声明并标明非官方。生成器当时的条款没有核到。这些原图不放进本仓库。本地调试包是临时制品，固定上游修订 `2d35234a7a01b18bdb3ad6c32e646b3d10e87828`，不作为官方资源发布。

如果你自己有权使用某套图，可以做成资源包，dst 会优先用它。运行时目录是 `DST_PET_PACK`，否则是 `~/.dst/pet-pack/`。

目录是 `~/.dst/pet-pack/`。至少要有 `idle.ppm`。其余文件名对应姿态：

`blink` `think` `talk` `happy` `sad` `eat` `blush` `sleep`

缺的姿态回落到 `idle.ppm`。品红 `#FF00FF` 是透明色。边长 1 到 1024 像素，P3 或 P6 PPM。dst 按最近邻缩进侧栏（约 56×64）和空闲房间（约 84×96）。

```bash
# 把你自己有权使用的 PNG 写成 P6。不要把参考仓的 webp 提交进这个仓库。
convert your-idle.png -depth 8 ~/.dst/pet-pack/idle.ppm
# 然后在 dst 里
/pet pack
```

没有这个目录时用字符鲸鱼娘。目录在但没有可读的 `idle.ppm` 时，`/pet` 会说明原因，画面仍是字符鲸鱼娘。

预览内置形象（不访问网络，也不写面板）：

```bash
DST_NO_UPDATE_CHECK=1 ./target/debug/dst --pet-preview sheet /tmp/pet-preview
DST_NO_UPDATE_CHECK=1 ./target/debug/dst --pet-preview room idle /tmp/room-idle.ppm
```

模拟器要带 `DST_NO_UPDATE_CHECK=1`。release 构建会在启动时检查更新，可能把二进制换成 nightly。
