# DMsender_CLI
DMsender_CLI是一个用于发送bilibili弹幕的命令行工具

# 使用方法
在终端中指定程序路径，回车后出现帮助信息即为成功

# 使用流程

## 任务文件创建
1. 使用`create`命令创建任务文件
```bash
# create 命令例：
.\DMsender.exe create "D:\sample.xml"
```
2. SESSDATA与bili_jct隐藏输入且没有退格删除，复制粘贴后回车
3. 输入视频信息，bv号与目标分p
4. 解析验证xml，若启用严格校验则剔除非法数据
5. 若有m7弹幕则查询是否有发送权限，若没有，可选择是否继续创建
6. 若指定输出路径则输出至指定路径，否则默认输出至可执行程序同目录下的`tasks`目录下，若没有指定文件名，则使用默认文件名格式，仅将以`/`或`\`结尾的指定路径视为目录，如：`-o "D:\tasks\"`若目录不存在或写入失败，则输出至默认路径

## 发送流程
1. 使用`send`命令选择任务文件进行发送
```bash
# send 命令例：
.\DMsender.exe send "D:\task_sample.json"
```
2. 询问是否继续上次的发送（若有），任务文件创建超过一周询问是否更新凭据
3. 询问发送间隔（单位秒，默认为10）
4. 开始发送
- 发送时：
  - 可在发送时打断，`p`键暂停，`q`键退出，暂停时使用`c`键继续
  - 返回错误代码为 -101 | -102 | -111 | 36705 时代表凭据可能过期或账号不可用
  - 返回错误代码为 -404 | 36700 | 36703 时可能是网络问题或触发了风控，进入重试流程
  - 返回错误代码为 -400 | 36701 | 36702 | 36706 | 36707 | 36708 | 36709 | 36710 | 36712 | 36714 | 36718 时可能是弹幕参数无效或无权限，进入修改流程
  - 返回错误代码为 36715 | 36704 | 36711 | 36713 时说明出现了致命错误，如视频信息错误或关闭了弹幕
5. 结束发送 => 退出程序

### 重试流程
1. 自动重试 5 次
2. 成功 => 下一条，5 次皆失败 => 询问继续重试或跳过

### 修改流程
1. 弹出三个选项，跳过/修改/退出
2. 上下方向键选择，使用回车选定，若`>`标识上下键无效请先回车一次再进行选择
3. 选择修改当前弹幕时依次输入mode、msg、progress、color、fontsize，不输入任何字符时回车使用原值

# 功能

## 根命令SubCommands:
- create  根据 XML 创建任务文件 用法: DMsender create "XMLFILEURL"
- send    选择任务文件发起网络请求发送弹幕 用法: DMsender send "TASKFILEURL"
- help    获取帮助

## 选项Options:
- -h, --help     Print help
- -V, --version  Print version

## create命令
**Usage**: 
- DMsender.exe create [OPTIONS] <XML_PATH>

**Arguments**:
-   <XML_PATH>  XML 文件的路径（本地路径或 URL）

**Options**:
| 名称 | 功能 | 简写 |
| --- | --- | --- |
| --rigor                    | 启用严格校验模式，过滤非法数据 | -r |
| --output <OUTPUT>          | 指定任务文件的输出路径 | -o |
| --sendafter                | 创建完成后直接启动发送流程 |   |
| --timeoffset <TIMEOFFSET>  | 对转换后的 progress 进行偏移（单位 ms，支持 +/-） |   |
| --auto                     | 自动模式：所有需要用户选择的地方自动跳过 （仅在同时指定 --sendafter 时有效） |   |
| --help                     | Print help | -h |

### 集大成者
```bash
.\DMsender.exe create "D:\sample.xml" -r --sendafter --auto --timeoffset=-10000 -o "C:\Users\username\Downloads\"
```

## send命令
**Usage**: 
- DMsender.exe send [OPTIONS] <TASK_PATH>

**Arguments**:
-   <TASK_PATH>  任务文件路径

**Options**:
- --auto  自动模式：
  - 错误类型处理：Retry 默认重试5次后跳过，Fatal/ReAuth 直接退出，Modify 直接跳过
- -h, --help  Print help

# 项目文件结构
```
│   .gitattributes
│   .gitignore
│   Cargo.lock
│   Cargo.toml
│   LICENSE
│   README.md
│
└───src
    │   actions.rs                #主行为逻辑
    │   errors.rs                 #错误处理
    │   main.rs                   #程序入口
    │
    ├───cli
    │       args.rs               #命令结构
    │       mod.rs
    │       prompt.rs             #提示选项
    │
    └───core
            api.rs                #网络api
            mod.rs
            task.rs               #任务文件结构
            wbi.rs                #wbi签名
            xml_parser.rs         #xml解析
```

# 安全性
任务文件中的SESSDATA使用明文存储，请妥善保管！！完成发送后建议即刻销毁

# LICENSE
MIT LICENSE
