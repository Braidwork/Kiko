[中文文档](./README_zh-CN.md)

## Import SDK

> Assume that the address of this tool is: http://127.0.0.1:12715

### script tag
```html
<script src="http://127.0.0.1:12715/assets/BraidworkKikoSDK.min.js"></script>
```

### ES6 module
```javascript
import { BraidworkKikoSDK } from 'http://127.0.0.1:12715/assets/BraidworkKikoSDK.min.js'
```

## How to use

> Let's take Vue3 as an example

```javascript
// Is it waiting for an answer
const waitingResponse = ref(false)
// User input
const userAsk = ref('')
// An array for storing chat records
const chatRecords = ref([])
// SDK variable
let BraidworkKikoSDK = null;
async function dryrun() {
    // If the user has no input, the answer API won't be requested
    if (chatRecords.value.length > 0 && !userAsk.value)
        return;
    // Check if it's waiting for the answer API to return, to avoid duplicate answers
    if (waitingResponse.value)
        return;
    waitingResponse.value = true;
    if (BraidworkKikoSDK == null) {
        braidworkKikoSDK = new BraidworkKikoSDK({
            // Make sure to replace <code>http://127.0.0.1:12715/flow/answer</code> with your actual API endpoint.
            url: 'http://127.0.0.1:12715/flow/answer', // answer API address
            robotId: robotId, // robot id
            mainFlowId: mainFlowId, // main flow id
            chatHistory: chatRecords.value, // Save chat data. This data, which is saved in the SDK
        });
    }
    // Sending request
    await braidworkKikoSDK.sendMessage({
        type: braidworkKikoSDK.MessageKind.PLAIN_TEXT,
        content: userAsk.value,
    });
    // Check if the answer is finished
    if (braidworkKikoSDK.chatHasEnded) {
        braidworkKikoSDK.addChat('Conversation was over', 'terminateText', braidworkKikoSDK.MessageKind.PLAIN_TEXT, -1);
        // When the session is complete, reset the SDK
        braidworkKikoSDK = null;
    }
    waitingResponse.value = false;
}
```
