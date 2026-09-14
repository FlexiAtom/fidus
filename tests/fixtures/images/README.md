# 可复现图片素材

这些 PNG 是 `tools/generate_test_images.py` 生成的、仅用于无显示测试的合成背景，**不依赖外部壁纸或版权素材**。生成器只使用 Python 标准库，固定尺寸 `128×96`、固定种子和 PNG 编码参数，生成结果由 `manifest.json` 中的 SHA-256 校验。

包含四种对照：

- `fractal.png`：有界 Mandelbrot escape-time 场；
- `texture.png`：整数哈希生成的多尺度宽带纹理；
- `gradient.png`：二维平滑渐变；
- `periodic.png`：两个不相干周期的正弦纹理。

重新生成并验证：

```sh
python3 tools/generate_test_images.py \
  --output tests/fixtures/images \
  --manifest tests/fixtures/images/manifest.json
python3 tools/test_generate_test_images.py
```

`manifest.json` 是测试素材的唯一登记入口；测试不会访问显示服务器，也不会读取工作站壁纸。
