# VRM 渲染依赖

使用 npm 精确版本 `@pixiv/three-vrm@3.5.5`、`three@0.180.0`，完整性摘要保存在 `pnpm-lock.yaml`；均为 MIT 许可。随目录保存安装包中的原始许可文本，开发类型依赖为 `@types/three@0.180.0`。

- three-vrm：https://github.com/pixiv/three-vrm/tree/v3.5.5
- Three.js：https://github.com/mrdoob/three.js/tree/r180

以本地依赖方式复用官方 GLTFLoader、VRM humanoid、MToon、表情、springbone 和资源销毁代码，没有 CDN 或远程脚本。应用层另做单文件离线导入、资源预算、角色库选择和暂停生命周期控制。

此目录的代码许可不授予任何 VRM 模型、Live2D Core SDK 或角色素材权利。验证用官方模型留在隔离的 output 目录，未随应用打包；模型导入后保留自己的 VRMC_vrm.meta 及其许可条件。
