# 鲸鱼娘立绘包

内置的是本仓原创的 28×32 像素女孩（随 `dst` 的 GPL-3.0 分发）。`dsh-whale-musume` 里的 webp 立绘没有放进本仓：代码的 MIT 文本没有单独授权那些图片，本次也没有取得作者许可。如果你自己有权使用某套图，可以做成资源包，dst 会优先用它。

目录是 `~/.dst/pet-pack/`。至少要有 `idle.ppm`。其余文件名对应姿态：

`blink` `think` `talk` `happy` `sad` `eat` `blush` `sleep`

缺的姿态回落到 `idle.ppm`。品红 `#FF00FF` 是透明色。边长 1 到 1024 像素，P3 或 P6 PPM。dst 按最近邻缩进侧栏（约 56×64）和空闲房间（约 84×96）。

```bash
# 把你自己有权使用的 PNG 写成 P6。不要把参考仓的 webp 提交进这个仓库。
convert your-idle.png -depth 8 ~/.dst/pet-pack/idle.ppm
# 然后在 dst 里
/pet pack
```

没有这个目录时用内置女孩。目录在但没有可读的 `idle.ppm` 时，`/pet` 会说明原因并继续用内置女孩。

预览内置形象（不访问网络，也不写面板）：

```bash
DST_NO_UPDATE_CHECK=1 ./target/debug/dst --pet-preview sheet /tmp/pet-preview
DST_NO_UPDATE_CHECK=1 ./target/debug/dst --pet-preview room idle /tmp/room-idle.ppm
```

模拟器要带 `DST_NO_UPDATE_CHECK=1`。release 构建会在启动时检查更新，可能把二进制换成 nightly。
