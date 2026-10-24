[English](./README.md)

## 引入SDK

> 假设本工具的地址是：http://127.0.0.1:12715

### script 标签
```html
<script src="http://127.0.0.1:12715/assets/BraidworkKikoSDK.min.js"></script>
```

### ES6 模块
```javascript
import { BraidworkKikoSDK } from 'http://127.0.0.1:12715/assets/BraidworkKikoSDK.min.js'
```

## 使用

> 我们以 Vue3 来做示例

```javascript
// 是否在等待答案
const waitingResponse = ref(false)
// 用户输入
const userAsk = ref('')
// 聊天记录
const chatRecords = ref([])
// SDK 变量
let braidworkKikoSDK = null;
async function dryrun() {
    // 如果用户没有输入，就不请求应答接口
    if (chatRecords.value.length > 0 && !userAsk.value)
        return;
    // 检查是否在等待应答接口返回，避免重复应答
    if (waitingResponse.value)
        return;
    waitingResponse.value = true;
    if (braidworkKikoSDK == null) {
        braidworkKikoSDK = new BraidworkKikoSDK({
            // 确保下面的地址 <code>http://127.0.0.1:12715/flow/answer</code> 替换成您真实的地址.
            url: 'http://127.0.0.1:12715/flow/answer', // 应答接口地址
            robotId: robotId, // 机器人id
            mainFlowId: mainFlowId, // 主流程id
            chatHistory: chatRecords.value, // 保存聊天记录数据。这些数据，是在SDK里保存的
        });
    }
    // 发送请求
    await braidworkKikoSDK.sendMessage({
        type: braidworkKikoSDK.MessageKind.PLAIN_TEXT,
        content: userAsk.value,
    });
    // 检查是否结束应答
    if (braidworkKikoSDK.chatHasEnded) {
        braidworkKikoSDK.addChat('会话结束', 'terminateText', braidworkKikoSDK.MessageKind.PLAIN_TEXT, -1);
        // 当会话完成，重置SDK
        braidworkKikoSDK = null;
    }
    waitingResponse.value = false;
}
```
