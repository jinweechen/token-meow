# 条件数值拟合组件

固定使用 Rust `nalgebra = 0.35.0`，关闭默认 features，仅启用 `std`，可在无桌面功能的构建中使用。Apache-2.0 许可原文保存在本目录 `LICENSE`。

官方来源：https://github.com/dimforge/nalgebra ，本地 crates.io 包内 `.cargo_vcs_info.json` 的固定 commit 为 `5f927f6c821d0ced07a0e086e5f37251cab2a8c3`；版本源：https://github.com/dimforge/nalgebra/tree/5f927f6c821d0ced07a0e086e5f37251cab2a8c3 。Cargo.lock 的包校验值为 `adc43a60c217b0c6ff46e47f26911015ad8d2e5a8be1af668c67e370d99a4346`。

本应用复用有迭代上限的 SVD 以及矩阵运算，求解最多三列的手动非负扣额情景。样本准入、秩与条件数、训练与留出隔离、显式未知状态由应用自行实现。数值可辨识或留出通过不代表官方账户身份、真实费率、扣额规则、完整调用覆盖或厂商额度证明，也不授予通知资格。
