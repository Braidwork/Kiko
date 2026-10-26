# 流式回答的线上协议

本文写在「流式回答链路重做」（帧定界、终帧统一、客户端可选）之后，给后续改动
`/flow/answer` 的人留一份可直接对照的资料。

结论先行：**只有一套协议，由请求体的 `stream` 字段选择**；`stream: true` 时响应是
`application/x-ndjson`，每行一个 JSON，**最后一行必定是携带 `{status, data, err}`
信封的终帧**。旧客户端不发这个字段，拿到的是与 1.22 及以前逐字节一致的普通 JSON。

---

## 1. 请求

`POST /flow/answer`（以及 `POST /flow/answer/multipart`）在原有请求体上新增一个字段：

| 字段 | 类型 | 缺省 | 含义 |
| --- | --- | --- | --- |
| `stream` | `bool` | `false` | 是否按帧推送回答 |

`#[serde(default)]`：**不发这个字段的老客户端拿到普通 JSON**。Python SDK、第三方内嵌的
旧 `DialogFlowAiSDK.js` 因此从「遇到流式流程直接坏掉」变成「正常工作，只是不流式」。

节点级的 `response_streaming` 保留，含义收窄为「该 LLM 节点是否按 token 逐块推送」，
且只在客户端要求流式时生效。它不再决定响应是不是流式的。

## 2. 响应

`Content-Type: application/x-ndjson`，`Cache-Control: no-cache`，
`X-Accel-Buffering: no`（nginx 默认会缓冲代理的分块响应，缺了这条整套流式在常见部署里
直接退化成「结尾一次性到达」），HTTP 状态恒为 200。

两种帧，靠 `contentSeq` 区分：

```jsonc
// 增量帧：contentSeq 是这条答案在会话上下文里的序号，按序拼接即得到该答案全文
{"contentSeq": 0, "content": "你"}
{"contentSeq": 0, "content": "好"}
{"contentSeq": 1, "content": "另一条答案"}

// 终帧：contentSeq 为 null，content 是整个响应文档（不是答案文本）
{"contentSeq": null, "content": "{\"status\":200,\"data\":{...},\"err\":null}"}
```

终帧的 `content` 与**非流式响应的 body 逐字节同构**，这样客户端不必按传输方式写两套解析。
失败时同样是终帧，`status` 是 500、`err` 里有 `message`：

```json
{"contentSeq": null, "content": "{\"status\":500,\"data\":null,\"err\":{\"message\":\"...\"}}"}
```

**为什么终帧必定是最后一个、且必定发送**：流程跑在一个自己的任务里，任务结束时才发终帧
并 drop sender，body 随之自然结束。节点返回 `true`（`ret: true` 的重量级 LLM 节点都这样）
不再是「不返回终帧」的路径 —— 这一点在 1.22 是坏的。

**为什么错误也在终帧里**：响应头在流程开跑之前就已经发出去了，HTTP 状态码在那时已经定死，
之后的任何失败都只能写在流里。

### 代理与缓冲

`X-Accel-Buffering: no` 只对 nginx 有效。其他反代/CDN 需要各自关掉缓冲，否则帧会被攒起来。
客户端侧无法察觉这种「假流式」：帧不会丢，只是晚到。

## 3. 服务端实现要点

| 位置 | 说明 |
| --- | --- |
| [`facade::answer`](../src/flow/rt/facade.rs) | 按 `req.stream` 分叉；流式时**在这里建 channel**，spawn 流程任务后立刻返回 body |
| [`server::to_ndjson`](../src/web/server.rs) | `UnboundedReceiverStream` → `Body::from_stream`，逐帧 `serde_json::to_string(&f) + "\n"` |
| [`executor::process_streaming`](../src/flow/rt/executor.rs) | 与非流式共用同一套循环，结束后 `push_terminal(envelope_json(r))` |
| [`dto::ResponseChannelWrapper`](../src/flow/rt/dto.rs) | 只持有 sender；`push_frame` / `push_terminal` 返回 `false` 表示客户端已走 |

channel 必须是**无界**的：`send()` 随之变成同步、有序、非阻塞，既不会因为读取方慢而反过来
限流生成，也不会出现 token 乱序。1.22 的每 token 一次 `spawn_blocking` 发送正是乱序的根因
（`spawn_blocking` 不保证执行顺序）。

**建 channel 的位置很关键**：放在 handler 里、spawn 之前。放在流程内部的话，在流程返回之前
没有任何人排空 channel，所有帧会堆到最后一齐冲出 —— 那就不是流式了。

### 取消

客户端断开 → 读端消失 → `push_frame` 返回 `false` → 生成循环立即 `break`。
这条检查覆盖所有 provider：`llama` / `phi3` / `gemma` / `moondream` 的本地生成、
以及 `chat` / `completion` 里的 OpenAI 兼容与 Ollama 分支，统一走
`ResultSender::push_delta`（[`src/ai/chat.rs`](../src/ai/chat.rs)）。

1.22 的问题：`phi3` 完全不检查（断连后仍跑完整个生成）；`gemma` / `moondream` 用 `try_send`
且**队列一满就中止生成**（当时是 `mpsc::channel(2)`，也就是生成两个 token 后中断）。

### provider 侧的增量解析

[`src/ai/stream.rs`](../src/ai/stream.rs) 的 `DeltaStream` 把 SSE / NDJSON 的 chunk 流切成
文本增量，四个 provider 分支共用。要点：

- 按 `0x0A` 切行。**`0x0A` 不可能是 UTF-8 多字节序列的一部分**，所以切在 chunk 中间也不会
  切出半个字符。
- OpenAI 兼容端点：`data:` 行累积，空行表示事件结束；`:` 开头是注释（OpenRouter 的
  `: OPENROUTER PROCESSING`、keep-alive）；`[DONE]` 结束；**以 `{` 开头的行视为完整文档**
  —— 同一份代码同时兼容 `text/event-stream` 与裸 `application/x-ndjson`（vLLM、llama.cpp、
  部分网关），不必嗅探 Content-Type。
- **一行坏数据只记日志，绝不中止整条流**。1.22 把 SSE 的字节片段当完整 JSON 解析，`?` 会让
  整个函数失败 —— 这就是「OpenAI 兼容 provider 推不出任何内容」的原因。

## 4. 客户端

| 客户端 | 做法 |
| --- | --- |
| Java（`sdk/java`） | `RequestData.setStream(true)`；`MappingIterator<StreamingResponseData>` 原生支持连续 JSON 值，无需自己分帧。带回调的 `req(rd, timeout, onChunk)` 逐 delta 回调，不带回调的签名会把 delta 聚合成完整 `answers` |
| JavaScript | `frameJson(state, text)` 维护 carry buffer + 括号计数切顶层 JSON 对象，忽略字符串内与转义后的花括号；任意 chunk 边界都成立 |
| Python | 不发 `stream`，拿到普通 JSON，行为与 1.22 一致 |

JS 侧只有一份可编辑源码：`frontend/src/assets/DialogFlowAiSDK.js`。
`frontend/public/assets/DialogFlowAiSDK.min.js` 是它经 `npm run build` 产出的发布产物，
`sdk/javascript/DialogFlowAiSDK.min.js` 是同一份产物的拷贝（两者当前逐字节相同）。改源码后要重新构建并同步，
**不要直接编辑压缩文件**。

`sdk/javascript/test/frameJson.test.mjs` 从**压缩产物**里抽出 `frameJson` 来跑，所以它验证的是真正发出去的那份：
定位函数结尾用的是忽略字符串字面量的花括号配对（函数体里全是 `'{'` / `'}'` 字符字面量，朴素计数会走穿），
随后对**每一个可能的切分点**各跑一遍，外加 1 字节粒度的分块喂入 ——
terser 改名形参、重写函数体之后分帧仍然正确，这件事由它保证。

Java 侧注意两点：

1. `HttpRequest.timeout` **只覆盖到响应头**（[JDK-8258397](https://bugs.openjdk.org/browse/JDK-8258397)，
   直到 JDK 25.0.3 才随 JDK-8208693 修复）。`ofInputStream()` 收头即返回，**读 body 期间
   毫无超时保护**，服务端卡住会永久阻塞调用线程。因此提供 `setIdleTimeoutMillis`（默认 0 =
   关闭）：超过 N 毫秒没有新帧就 close 掉 body 流。close 会往读线程阻塞的队列里塞一个哨兵，
   所以阻塞的 read 会醒来并抛错。
2. `stream` 是原始类型 `boolean`，永远参与序列化 —— 请求方明确说清要哪一种，而不是靠字段
   缺失去猜。

## 5. 流式答案的历史回填

`chat_history` 是下一次 LLM 调用读到的上下文，所以每个答案都必须落进去 —— 但流式答案在
生成**开始前**就得占好位置（帧是一个 token 一个 token 出去的，那时还不知道全文）。因此拆成
两步（[`Context`](../src/flow/rt/context.rs)）：

1. `add_answer_history("")` 预留槽位，返回 `AnswerSlot`；
2. 生成结束后 `fill_answer_history(slot, &answer)` 填入。

节点结束但**什么都没发出去**时（`GotoAnotherNode`、`DoNothing`、空回答），调
`discard_answer_history(slot)` 把预留收回 —— 一条空的助手消息比没有更糟，它就是下一次
LLM 调用会读到的东西。收回只在槽位仍是最新时才生效，所以过期槽位不会误删别人的消息。

### `AnswerSlot` 的两个数字为什么不相等

`content_seq` 是 `chat_history.len() - 1`，`idx` 是 push 后的真实下标，**差一**。这不是笔误，
是**必须保留的历史包袱**：所有已发布的客户端都按 `content_seq` 的值渲染。

后果是 JS 侧（`addChat` 里的 `if (idx && idx > -1)`）行为如下：

| | 服务端 `content_seq` | JS 的落点 |
| --- | --- | --- |
| 第 1 个答案 | `0` | `0` 是 falsy → 新建消息（结果正确） |
| 第 2 个答案 | `1` | 追加到 `chatHistory[1]`，也就是**第 1 个答案上**（错误） |
| 第 3 个答案 | `2` | `2 >= chatHistory.length` → 先补一条空消息，再新建（多一条空气泡） |

单答案的流程看不出问题（绝大多数流程都是），多答案的流程会错。要修的话只需让
`add_answer_history` 返回 push 后的下标（即 `content_seq: idx`）—— 上表三行会同时变正确，
JS 侧一行都不用动 —— 但那会改变线上 `contentSeq` 的取值，且属于另一个改动，
单独做、单独验。

### 仍然不在本轮范围内

1. **客户端断连时，非生成类节点（如外部 HTTP 调用）不会立即中止**。只有生成循环检查了
   sender 是否还在。
2. **本地 HF 模型的 `gen_text` 是同步阻塞函数**，`await` 它会在整个生成期间占住一个 runtime
   worker 线程。这是现状（1.22 的 `tokio::task::spawn` 同样阻塞 worker），不是本轮引入。
   彻底解决要包一次 `spawn_blocking`，但那样需要调整 `ResultSender` 的生命周期
   （`StrBuf(&mut String)` 借用了外部栈、跨不进 `spawn_blocking`）。注意必须是**整次生成一次**，
   绝不能回到 per-token。

## 6. 下一轮

服务端遗留的两条在上面，本节是**已经决定要做、但还没开工**的客户端改动。写下来是为了下一轮不必
重新发现它们。

### 6.1 Python SDK 支持流式

§4 里那句「Python 不发 `stream`，拿到普通 JSON，行为与 1.22 一致」记的是**当时的现状，不是结论**。
Python 一旦支持流式，它就要改成和 Java 同一套说法：`stream: true` → 按 `contentSeq` 聚合增量帧 →
最后解析终帧的信封。三个要点：

- Python 没有 `MappingIterator` 那种「连续 JSON 值」的读者。`json.JSONDecoder.raw_decode` 配一个
  游标可以在缓冲区上循环切帧，比按 `\n` 手工切更稳 —— 协议保证帧以 `\n` 结尾，但**不保证**一个
  TCP chunk 里刚好是整帧，所以无论如何都要带 carry buffer。
- 终帧的 `content` 是**字符串**，不是嵌套对象：要 `json.loads` 第二次才拿到 `{status,data,err}`。
  流式时 `data.answers` 必定为空（见 §5），不能拿它当答案来源。
- 超时：`requests` 的 `timeout` 在 `stream=True` 下是**每次 read 之间**的超时，不像 Java 的
  `HttpRequest.timeout` 只覆盖到响应头（§4）。落地时实测确认一次，然后把 §4 的 Java 注意事项
  对照着写 Python 那一段。

### 6.2 流程编辑器测试窗暴露流式开关

`SubFlow.vue` 的测试窗要能选「是否流式」。开工前先看 `dryrun2`（约 834 行起）：它调用 `chatReq`，
但该文件只 import 了 `{ atob, httpReq }`（约 24 行），**是死代码，一跑就 ReferenceError**。
测试窗的流式开关正好落在这一段，所以先把 `chatReq` 和 SDK 的真实导出对齐，再谈加开关。

另外，`frontend/src/assets/DialogFlowAiSDK.js` 是 JS 侧**唯一可编辑的源码**（§4），测试窗要用到流式
回调时改它，改完 `npm run build`，并把产物同步到 `sdk/javascript/DialogFlowAiSDK.min.js`。
