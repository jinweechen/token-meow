# 二维物理求解组件

固定使用 Rust `rapier2d = 0.36.1`，启用 `dim2/f32/std/block-solver`，不启用 parallel、FEM、JS/WASM、Bevy 或 SIMD8。其 Apache-2.0 原文保存在本目录 `LICENSE`；精确依赖及校验值见 Cargo.lock。

官方固定源：https://github.com/dimforge/rapier/tree/v0.36.1 ，tag commit `3406750a38286a0d3c7d0f4be2f9d53153e81d90`。

本应用只复用有界刚体、碰撞与时间步求解。Rapier 不拥有操作系统窗口权限，所有位移仍由宿主原生许可和最新事实、显示区域、姿态确认共同决定。库内仿真结果不代表 Windows 原生移动验收。

其它开源桌宠仅用于架构研究，没有将其角色素材或物理代码复制到本应用；模型/图像许可按各自资源声明处理。
