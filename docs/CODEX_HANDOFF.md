# BackupTool 项目移交说明

## 1. 项目背景

本项目用于《软件开发综合实验》，目标是实现一款数据备份软件。

课程基本功能要求：

- 数据备份：将目录树中的文件数据保存到指定位置。
- 数据还原：将目录树中的文件数据恢复到指定位置。

课程扩展功能包括：

- 特殊文件支持。
- 文件元数据支持。
- 自定义备份筛选：路径、类型、名字、时间、尺寸、用户。
- 打包与解包。
- 压缩与解压。
- 加密与解密。
- GUI。
- 定时备份。
- 实时备份。
- 网络备份。

项目要求采用面向对象的软件工程方法，经历需求分析、系统设计、编码实现和软件测试完整生命周期。

课程开发语言要求允许 C/C++ 等语言，GUI 可以采用脚本语言编写。对扩展功能，如果直接使用第三方库、程序或代码完成，对应扩展分可能折半。

因此，本项目采用以下总体策略：

> C++ 实现绝大多数核心业务逻辑；Tauri 2 提供 GUI 和桌面应用框架；Rust 只作为 Tauri 与 C++ 之间的薄适配层。

---

## 2. 总体技术架构

```text
┌──────────────────────────────────┐
│ Frontend / GUI                   │
│ TypeScript + HTML + CSS          │
│ Tauri WebView                    │
└────────────────┬─────────────────┘
                 │ invoke / event / channel
                 ▼
┌──────────────────────────────────┐
│ Rust Adapter                     │
│                                  │
│ 仅负责：                         │
│ - Tauri command                  │
│ - 参数转换                       │
│ - 返回值转换                     │
│ - C/C++ FFI 调用                 │
│ - 必要的事件转发                 │
└────────────────┬─────────────────┘
                 │ C ABI / FFI
                 ▼
┌──────────────────────────────────┐
│ C++ Backup Core                  │
│                                  │
│ FileScanner                      │
│ FileFilter                       │
│ BackupManager                    │
│ RestoreManager                   │
│ Archiver                         │
│ Compressor                       │
│ Encryptor                        │
│ Scheduler                        │
│ FileWatcher                      │
└──────────────────────────────────┘
```

核心原则：

1. C++ Core 必须可以脱离 Tauri 独立构建。
2. C++ Core 必须可以脱离 GUI 独立测试。
3. Rust 不实现主要业务逻辑。
4. GUI 不直接承担备份算法、压缩算法、打包算法等核心逻辑。
5. Rust Adapter 应保持尽可能薄。
6. GUI 需求可以影响 C++ API 设计，因此必须尽早打通一次完整调用链。
7. 不应先完整实现所有 C++ 功能再首次接入 Tauri。
8. 不应先花大量时间完善 GUI 再开发 C++ Core。

---

## 3. 推荐项目目录

建议采用如下目录结构：

```text
BackupTool/
├── src/
│   ├── ...
│   └── 前端 TypeScript / HTML / CSS
│
├── src-tauri/
│   ├── Cargo.toml
│   ├── build.rs
│   └── src/
│       ├── lib.rs
│       ├── commands/
│       └── ffi/
│
├── core/
│   ├── CMakeLists.txt
│   ├── include/
│   │   ├── backup/
│   │   │   ├── BackupManager.hpp
│   │   │   ├── BackupConfig.hpp
│   │   │   ├── BackupProgress.hpp
│   │   │   ├── BackupResult.hpp
│   │   │   ├── RestoreManager.hpp
│   │   │   ├── FileScanner.hpp
│   │   │   └── FileFilter.hpp
│   │   │
│   │   └── backup_c_api.h
│   │
│   ├── src/
│   │   ├── BackupManager.cpp
│   │   ├── RestoreManager.cpp
│   │   ├── FileScanner.cpp
│   │   ├── FileFilter.cpp
│   │   └── backup_c_api.cpp
│   │
│   └── tests/
│       └── ...
│
├── CMakeLists.txt
├── package.json
└── README.md
```

不要把 C++ Core 放到：

```text
src-tauri/cpp/
```

原因：

> `core` 应作为独立业务核心存在，而不是被设计成 Tauri 的附属模块。

---

## 4. 第一阶段唯一目标：打通最小调用链

第一阶段不要真正实现备份功能。

目标仅为：

```text
GUI
 ↓
TypeScript invoke()
 ↓
Tauri Rust command
 ↓
Rust FFI
 ↓
C ABI
 ↓
C++
 ↓
返回字符串
 ↓
Rust
 ↓
TypeScript
 ↓
GUI 显示结果
```

建议使用以下最小验证功能：

```text
GUI 点击按钮
    ↓
调用 core_version
    ↓
C++ 返回 "Backup Core 0.1.0"
    ↓
GUI 显示 "Backup Core 0.1.0"
```

一旦此调用链成功，第一阶段立即结束。

禁止继续在此阶段研究或实现：

- 自定义标题栏。
- 窗口动画。
- Tray。
- Updater。
- Window effects。
- 复杂前端状态管理。
- GUI 美化。
- 完整设置页面。
- Tauri 插件体系扩展。

---

## 5. 第一阶段 C++ 最小实现

### 5.1 C++ 内部接口

文件：

```text
core/include/backup/version.hpp
```

建议内容：

```cpp
#pragma once

#include <string>

namespace backup {

std::string GetCoreVersion();

}
```

文件：

```text
core/src/version.cpp
```

建议内容：

```cpp
#include "backup/version.hpp"

namespace backup {

std::string GetCoreVersion()
{
    return "Backup Core 0.1.0";
}

}
```

---

## 6. C ABI Adapter

Rust 不应直接绑定 C++ 类。

第一版必须通过 C ABI Adapter 暴露接口。

文件：

```text
core/include/backup_c_api.h
```

建议形式：

```cpp
#pragma once

#ifdef __cplusplus
extern "C" {
#endif

const char* backup_core_version();

#ifdef __cplusplus
}
#endif
```

文件：

```text
core/src/backup_c_api.cpp
```

建议形式：

```cpp
#include "backup_c_api.h"
#include "backup/version.hpp"

#include <string>

const char* backup_core_version()
{
    static const std::string version =
        backup::GetCoreVersion();

    return version.c_str();
}
```

第一阶段只需要一个接口：

```text
backup_core_version
```

---

## 7. Rust Adapter 设计要求

Rust 仅作为桥接层。

第一阶段需要：

```text
unsafe extern "C"
```

声明：

```rust
unsafe extern "C" {
    fn backup_core_version()
        -> *const std::ffi::c_char;
}
```

然后提供 Tauri command：

```text
core_version
```

逻辑：

1. 调用 `backup_core_version`。
2. 检查返回指针是否为空。
3. 使用 `CStr` 转换。
4. 转换为 Rust `String`。
5. 返回前端。

Rust command 可采用：

```rust
#[tauri::command]
fn core_version() -> Result<String, String>
```

不要在 Rust 中重新实现版本逻辑。

---

## 8. 前端第一阶段要求

GUI 仅需要：

```text
[ Test C++ Core ]
```

按钮。

点击后调用：

```ts
invoke<string>("core_version")
```

页面显示：

```text
Backup Core 0.1.0
```

仅需证明完整调用链有效。

不要制作正式 GUI。

---

## 9. 调用链成功后的开发顺序

完整调用链成功后：

> 暂停 Tauri 框架开发，将主要开发重心转到 C++ Core。

推荐顺序：

```text
FileScanner
    ↓
BackupManager
    ↓
基本普通文件备份
    ↓
RestoreManager
    ↓
基本普通文件恢复
```

第一条完整业务链目标：

源目录：

```text
source/
├── a.txt
├── dir/
│   └── b.txt
└── image.png
```

执行：

```text
Backup(source, backup)
```

随后：

```text
Restore(backup, restore)
```

最终：

```text
restore/
├── a.txt
├── dir/
│   └── b.txt
└── image.png
```

恢复后的目录树和文件内容应与源目录一致。

---

## 10. C++ Core 第一阶段业务模块

初步建议：

### FileScanner

负责：

- 遍历目录树。
- 获取普通文件。
- 获取目录。
- 为后续特殊文件支持预留接口。

不负责：

- 文件复制。
- GUI。
- 日志展示。
- Tauri。

### BackupManager

负责：

- 执行备份流程。
- 协调 FileScanner。
- 将目录结构和文件复制到备份位置。
- 返回 BackupResult。

### RestoreManager

负责：

- 根据备份数据恢复目录结构。
- 恢复文件内容。
- 返回 RestoreResult。

### BackupConfig

应逐渐承载：

- 源路径。
- 目标路径。
- 筛选规则。
- 打包选项。
- 压缩选项。
- 加密选项。

第一阶段只保留：

```text
source
destination
```

### BackupProgress

即使第一阶段暂时不实现异步进度反馈，也应预留合理设计。

建议字段：

```cpp
struct BackupProgress
{
    std::uint64_t total_files;
    std::uint64_t processed_files;

    std::uint64_t total_bytes;
    std::uint64_t processed_bytes;

    std::filesystem::path current_file;
};
```

原因：

GUI 后续需要：

- 显示扫描状态。
- 显示当前文件。
- 显示文件进度。
- 显示字节进度。
- 支持取消。
- 可能支持暂停和继续。

因此不要将核心接口永久设计成一个完全不可观察的：

```cpp
void Backup(...);
```

---

## 11. C++ API 设计约束

避免第一版直接设计为：

```cpp
void Backup(
    const std::filesystem::path& source,
    const std::filesystem::path& destination
);
```

长期接口更适合围绕任务对象设计，例如：

```cpp
class BackupTask
{
public:
    void Start();
    void Pause();
    void Resume();
    void Cancel();

    BackupProgress GetProgress() const;
};
```

但第一版不要急于一次完成完整异步任务模型。

推荐演进顺序：

```text
同步 BackupManager
    ↓
BackupResult
    ↓
Progress callback
    ↓
Cancel
    ↓
异步 Task
    ↓
Pause / Resume
```

遵循：

> 先完成可工作的最小模型，再逐步扩展。

---

## 12. C++ 测试要求

C++ Core 必须独立测试。

不启动：

- Tauri。
- Node.js。
- GUI。

即可运行测试。

建议使用 gtest。

第一批测试：

### FileScanner

- 空目录。
- 单文件目录。
- 多级目录。
- 深层嵌套。
- 文件名包含空格。
- Unicode 文件名。

### BackupManager

- 普通目录完整备份。
- 空目录备份。
- 目标目录不存在。
- 部分路径不存在。
- 文件复制失败。
- 源和目标路径冲突。

### RestoreManager

- 基本恢复。
- 恢复目录结构。
- 恢复文件内容。
- 目标已存在。
- 备份不完整。
- 备份文件损坏。

第一条验收原则：

```text
Backup(source)
    ↓
Restore(backup)
    ↓
Compare(source, restored)
```

比较至少包括：

- 目录结构。
- 文件数量。
- 文件尺寸。
- 文件内容。

---

## 13. 功能开发 Milestone

### Milestone 0：框架验证

实现：

```text
GUI → Rust → C++ → GUI
```

接口：

```text
core_version
```

完成后停止框架研究。

---

### Milestone 1：基本备份与恢复

C++：

- FileScanner。
- BackupManager。
- RestoreManager。
- 普通文件。
- 普通目录。

Rust：

- `backup`
- `restore`

GUI：

- 选择源目录。
- 选择备份目录。
- 开始备份。
- 选择备份。
- 选择恢复目录。
- 开始恢复。
- 显示结果。

---

### Milestone 2：自定义备份筛选

C++：

- 路径筛选。
- 文件类型筛选。
- 文件名筛选。
- 时间筛选。
- 文件尺寸筛选。
- 用户筛选。

建议：

```text
FileFilter
```

作为独立模块。

Rust：

- 参数 DTO 转换为 C++ 配置。

GUI：

- 筛选规则配置。

---

### Milestone 3：打包与解包

C++：

- 自定义 Archiver。
- 自定义 Unarchiver。

重点：

- 文件头设计。
- 文件目录表。
- 文件偏移。
- 文件长度。
- 路径信息。
- 数据区。

GUI：

- 是否打包。
- 打包算法选择。

---

### Milestone 4：压缩与解压

C++：

- Compressor 接口。
- Decompressor 接口。
- 至少一种自行实现的算法。

建议先根据课程要求确定算法。

GUI：

- 压缩启用。
- 算法选择。
- 压缩级别（若支持）。

---

### Milestone 5：加密与解密

C++：

- Encryptor。
- Decryptor。

注意：

课程评分意义下的“算法实现”和真实产品安全要求不同。

真实安全软件不应自行设计密码算法。

项目需要在课程评分要求和真实工程安全原则之间明确区分。

---

### Milestone 6：GUI 完善

基础业务功能稳定后再开发：

- 任务列表。
- 进度条。
- 当前文件。
- 速度。
- 剩余量。
- 错误提示。
- 历史记录。
- 配置页面。

---

### Milestone 7：定时备份

C++：

```text
Scheduler
```

负责：

- 周期配置。
- 下次执行时间。
- 备份触发。
- 数据淘汰。

Tauri：

- GUI 配置。
- 生命周期衔接。

---

### Milestone 8：实时备份

C++：

```text
FileWatcher
```

负责：

- 感知文件变化。
- 产生文件变化事件。
- 触发备份策略。

注意平台差异：

```text
Windows
Linux
```

底层文件变化 API 不同。

应通过抽象接口隔离。

---

## 14. 前后端集成节奏

不要：

```text
写一个 C++ 函数
↓
立即制作 GUI
↓
再写一个 C++ 函数
↓
立即制作 GUI
```

推荐按纵向 Milestone 集成。

例如：

```text
Milestone 1 C++ 完成
        ↓
C++ 单元测试通过
        ↓
添加 Rust Adapter
        ↓
添加 GUI
        ↓
集成测试
```

然后进入 Milestone 2。

---

## 15. FFI 边界原则

Rust 与 C++ 之间通过 C ABI。

避免直接暴露：

- `std::string`
- `std::vector`
- `std::filesystem::path`
- C++ class
- template
- exception

跨 FFI 边界。

C API 应使用：

- 基础整数类型。
- `const char*`
- POD struct。
- opaque handle。
- callback function pointer。

例如：

```cpp
typedef struct BackupResultC
{
    int success;
    uint64_t file_count;
    uint64_t byte_count;
    const char* error_message;
} BackupResultC;
```

复杂对象建议使用 opaque handle：

```cpp
typedef void* BackupTaskHandle;
```

例如：

```cpp
BackupTaskHandle backup_task_create(...);

int backup_task_start(
    BackupTaskHandle handle
);

int backup_task_cancel(
    BackupTaskHandle handle
);

void backup_task_destroy(
    BackupTaskHandle handle
);
```

C++ exception 不允许越过 FFI 边界。

C Adapter 必须：

```text
try
    ↓
调用 C++ Core
    ↓
catch
    ↓
转换为 error code / error message
```

---

## 16. 错误处理原则

C++ Core：

```text
C++ error model
```

可以采用：

- exception。
- `std::expected`，若编译标准支持。
- 自定义 Result。

但 C ABI 必须转为稳定形式。

Rust Adapter：

```rust
Result<T, String>
```

第一阶段足够。

GUI：

使用 rejected Promise 处理错误。

不要：

```cpp
std::cout << "error";
```

然后让 GUI 无法获得失败原因。

不要把业务错误只写到日志。

错误必须可以沿：

```text
C++
 ↓
C ABI
 ↓
Rust Result
 ↓
Tauri
 ↓
TypeScript
 ↓
GUI
```

完整传播。

---

## 17. 日志原则

C++ Core 不应直接依赖 GUI。

不要：

```cpp
ui->ShowMessage(...);
```

可以设计：

```text
Logger interface
callback
event sink
```

第一阶段允许简单日志。

后续建议：

```cpp
enum class LogLevel
{
    Debug,
    Info,
    Warning,
    Error
};
```

Rust 可以选择：

- 转发至 Tauri event。
- 写日志文件。
- 输出调试日志。

---

## 18. Windows 与 Linux 开发环境

主开发环境：

```text
Windows
```

原因：

- Tauri GUI 最终直接运行在 Windows WebView2。
- Windows 原生文件路径和文件系统行为需要直接测试。
- Tauri Windows 构建更自然。
- 避免 WSLg 增加 GUI 环境复杂度。

Linux / WSL：

```text
Linux 兼容性测试
Linux 构建
Linux 专属功能测试
```

推荐：

```text
Windows:
C:\Dev\BackupTool

WSL:
/home/<user>/dev/BackupTool
```

分别 clone Git 仓库。

不要共享：

```text
node_modules/
target/
```

不要推荐 Windows 和 WSL 同时在：

```text
/mnt/c/...
```

运行同一套构建缓存。

---

## 19. Windows 工具链建议

推荐：

```text
Rust stable-msvc
MSVC Build Tools
Node.js
Tauri 2
CMake
```

Rust：

```text
stable-x86_64-pc-windows-msvc
```

不要因为已有 MinGW 使用经验而优先采用 Rust GNU toolchain。

C++ Core 也应优先考虑与 MSVC / Tauri Rust MSVC 构建链兼容。

---

## 20. Codex 第一阶段任务

请严格按以下顺序实施。

### Task 1

检查现有仓库。

输出：

- 当前目录结构。
- 已有代码。
- 已有构建系统。
- 是否已有 Tauri 项目。
- 是否已有 CMake。
- 当前 Git 状态。

不要立即大规模修改。

### Task 2

若不存在 Tauri 2 工程：

创建最小 Tauri 2 工程。

要求：

- GUI 技术保持简单。
- 不引入大型前端框架，除非现有仓库已使用。
- 不进行 GUI 美化。

### Task 3

创建：

```text
core/
```

建立独立 CMake C++ Core。

实现：

```text
backup::GetCoreVersion()
```

返回：

```text
Backup Core 0.1.0
```

### Task 4

创建：

```text
backup_c_api.h
backup_c_api.cpp
```

暴露：

```text
backup_core_version
```

### Task 5

使 Rust 正确链接 C++ Core。

要求：

- 当前首先支持 Windows MSVC。
- 构建逻辑清晰。
- 不手工复制临时库文件作为最终方案。
- 将构建关系写入构建脚本。

### Task 6

实现 Rust command：

```text
core_version
```

Rust 通过 FFI 调用：

```text
backup_core_version
```

### Task 7

前端添加一个测试按钮：

```text
Test C++ Core
```

点击后：

```text
invoke("core_version")
```

页面显示：

```text
Backup Core 0.1.0
```

### Task 8

确认以下调用链成功：

```text
GUI
 ↓
TypeScript
 ↓
Tauri
 ↓
Rust
 ↓
C ABI
 ↓
C++
 ↓
Rust
 ↓
TypeScript
 ↓
GUI
```

### Task 9

完成后停止。

不要继续实现：

- FileScanner。
- BackupManager。
- RestoreManager。
- 正式 GUI。
- 压缩。
- 加密。
- 打包。
- Scheduler。
- FileWatcher。

### Task 10

最终报告：

1. 修改了哪些文件。
2. 最终目录结构。
3. CMake 如何构建 C++ Core。
4. Rust 如何链接 C++。
5. FFI 调用链如何工作。
6. Windows 下如何运行。
7. 已验证的结果。
8. 当前遗留问题。

---

## 21. Codex 实施约束

必须遵守：

1. 不重写已有有效代码，除非确有必要。
2. 修改前先检查仓库。
3. 小步修改。
4. 每次修改保持可构建。
5. 不提前实现未来 Milestone。
6. 不把核心业务逻辑写进 Rust。
7. 不把核心业务逻辑写进 TypeScript。
8. C++ Core 保持独立。
9. Rust 保持薄适配层。
10. FFI 使用 C ABI。
11. 异常不能越过 FFI。
12. 不过度设计。
13. 不提前实现完整异步任务系统。
14. 不进行无关重构。
15. 不添加与第一阶段目标无关的依赖。

---

## 22. 第一阶段验收标准

第一阶段仅在以下条件全部满足时视为完成：

- Tauri 2 应用可以运行。
- C++ Core 可以独立构建。
- Rust 可以链接 C++ Core。
- C ABI 正常工作。
- 前端可以调用 `core_version`。
- GUI 显示 `Backup Core 0.1.0`。
- C++ 函数确实被调用，而不是 Rust 返回硬编码字符串。
- 项目结构清晰。
- 构建方法已有文档。
- 没有提前实现大规模业务功能。

最终验收结果：

```text
[ Test C++ Core ]
        ↓

Backup Core 0.1.0
```

达到此结果后：

> 停止框架开发，下一阶段开始设计和实现 C++ FileScanner 与基础备份/恢复业务链。
