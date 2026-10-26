"use strict";
class BraidworkKikoSDK {
    constructor(options) {
        this.url = options.url;
        this.timeoutSec = options.timeoutSec || 10;
        this.mainFlowId = options.mainFlowId;
        this.robotId = options.robotId;
        this.chatHistory = options.chatHistory || [];
        this.version = 1;
        this.sessionId = options.sessionId || this.newSessionId();
        this.importVariables = [];
        this.attachments = [];
        this.chatHasEnded = false;
    }

    VarKind = Object.freeze({
        STRING: 'String',
        NUMBER: 'Number',
    });

    UserInputResult = Object.freeze({
        SUCCESSFUL: 'Successful',
        TIMEOUT: 'Timeout',
    });

    MessageKind = Object.freeze({
        PLAIN_TEXT: 'PlainText',
        RICH_TEXT: 'RichText',
        IMAGE: 'Image',
    });

    newSessionId() {
        const d = Date.now().toString();
        return d + Math.random().toString(16);
    }

    genRequestBody(userInput, userInputIntent) {
        const self = this;
        const body = {
            robotId: self.robotId,
            mainFlowId: self.mainFlowId,
            sessionId: this.sessionId,
            userInputResult: null,
            userInput: userInput || "",
            attachments: self.attachments.splice(0, self.attachments.length),
            importVariables: self.importVariables.splice(0, self.importVariables.length),
            userInputIntent: userInputIntent,
            // Ask for the answer frame by frame. It costs nothing when the flow
            // sends only one answer: the terminal frame carries it whole.
            stream: true
        };
        return body;
    }

    // attachment: { mimeType: 'image/jpeg', data: '<base64 or https URL>' }
    // or an image URL string, which is converted automatically.
    appendAttachment(attachment) {
        if (typeof attachment === 'string') {
            attachment = { mimeType: 'image/jpeg', data: attachment };
        }
        if (!attachment || !attachment.data) {
            throw new Error('Invalid attachment: missing data');
        }
        if (!attachment.mimeType) {
            attachment.mimeType = 'image/jpeg';
        }
        this.attachments.push(attachment);
    }

    appendImportVariable(name, value, kind) {
        const varKind = this.VarKind[kind];
        if (!varKind) {
            throw new Error(`Invalid variable kind: ${kind}`);
        }
        const variable = {
            varName: name,
            varType: kind,
            varVal: value,
        };
        this.importVariables.push(variable);
    }

    correctData(data) {
        if (!this.url) {
            throw new Error(`Missing parameter: url`);
        }
        if (!this.robotId) {
            throw new Error('Missing parameter: robotId');
        }
        if (!this.mainFlowId) {
            throw new Error('Missing parameter: mainFlowId');
        }
        if (data.sessionId == null)
            throw new Error('Missing parameter: sessionId');
        if (data.userInput == null)
            data.userInput = '';
        if (data.userInputResult == null)
            data.userInputResult = this.chatHistory.length == 0 || data.userInput.length > 0 ? this.UserInputResult.SUCCESSFUL : this.UserInputResult.FAILED;
        if (data.importVariables == null)
            data.importVariables = [];
        if (data.attachments == null)
            data.attachments = [];
        if (data.userInputIntent != null && data.userInputIntent == '')
            data.userInputIntent = null;
    }

    addChat(t, tS, aT, idx) {
        if (idx && idx > -1) {
            if (idx >= this.chatHistory.length) {
                for (let i = this.chatHistory.length; i < idx; i++) {
                    this.chatHistory.push({
                        id: 'chat-' + Math.random().toString(16),
                        text: '',
                        textSource: tS,
                        answerType: aT,
                    });
                }
            } else {
                this.chatHistory[idx].text += t;
                return idx;
            }
        }
        this.chatHistory.push({
            id: 'chat-' + Math.random().toString(16),
            text: t.trimStart(),
            textSource: tS,
            answerType: aT,
        });
        return this.chatHistory.length - 1;
    }

    appendAnswers(r, idx) {
        console.log(r);
        if (r.status == 200) {
            console.log('data.nextAction:', r.data.nextAction);
            const data = r.data;
            const answers = data.answers;
            let newIdx = -1;
            if (answers != null) {
                for (let i = 0; i < answers.length; i++)
                    newIdx = this.addChat(answers[i].content, 'responseText', answers[i].contentType, idx);
            }
            if (data.nextAction === 'Terminate')
                this.chatHasEnded = true;
            return { chatIdx: newIdx };
        } else {
            throw new Error(`Error: ${r.err.message}`);
        }

    }

    async sendMessage(message) {
        const self = this;

        // message.attachments: [{ mimeType, data }] or image URL strings.
        const messageAttachments = message.attachments || [];
        for (const a of messageAttachments)
            self.appendAttachment(a);

        // 构造请求体
        const body = self.genRequestBody(message.content, null);
        self.correctData(body);

        if (message.content || body.attachments.length > 0) {
            const chatIdx = self.addChat(message.content || '', 'userText', message.type, -1);
            if (body.attachments.length > 0) {
                const record = self.chatHistory[chatIdx];
                record.images = body.attachments
                    .filter((a) => a.mimeType && a.mimeType.startsWith('image/'))
                    .map((a) => a.data.startsWith('http') ? a.data : `data:${a.mimeType};base64,${a.data}`);
            }
        }
        // const res = {
        //     type: self.MessageKind.PLAIN_TEXT,
        //     content: '......',
        //     from: 'bot',
        //     timestamp: new Date().toISOString()
        // };
        // self.chatHistory.push(res);

        var controller = new AbortController();
        var timeoutId = setTimeout(function () {
            controller.abort();
        }, self.timeoutSec * 1000);

        console.log('Request body:', JSON.stringify(body));

        const response = await fetch(self.url, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json'
            },
            body: JSON.stringify(body),
            signal: controller.signal
        });

        clearTimeout(timeoutId);
        console.log('Response:', response);
        if (!response.ok) throw new Error('Network response was not ok');
        const contentType = response.headers.get('content-type') || '';

        const isStream = contentType.includes('text/event-stream') ||
            contentType.includes('application/x-ndjson') ||
            contentType.includes('text/plain');

        if (isStream) {
            const reader = response.body.getReader();
            const decoder = new TextDecoder('utf-8');
            const framer = { carry: '' };
            let idx = -1;
            const handle = (text) => {
                for (const frame of frameJson(framer, text)) {
                    console.log('frame:', frame);
                    const j = JSON.parse(frame);
                    if (Object.hasOwn(j, 'contentSeq') && j.contentSeq !== null) {
                        self.appendAnswers({ status: 200, data: { answers: [{ content: j.content }] } }, j.contentSeq);
                    } else {
                        // Terminal frame: its content is the whole response, so the
                        // final nextAction and collectData arrive with it.
                        const r = self.appendAnswers(JSON.parse(j.content), idx);
                        idx = r.chatIdx;
                    }
                }
            };

            for (; ;) {
                const { value, done } = await reader.read();
                if (done) break;
                console.log('chunk:', value);
                // `stream: true` keeps a character split across two chunks whole.
                handle(decoder.decode(value, { stream: true }));
            }
            // Flush whatever the decoder was still holding.
            handle(decoder.decode());
        } else {
            const res = await response.json();
            console.log('Response data:', res);
            self.appendAnswers(res, -1);
        }
    };
}

// Splits a stream of newline-delimited JSON into whole documents.
//
// A chunk can end anywhere — mid-frame, mid-string, even mid-escape — so the
// unfinished tail is carried over to the next call and the scan ignores braces
// that are inside a string literal. It also does not care whether a document is
// the only thing in a chunk, or shares one with three others.
function frameJson(state, text) {
    const buf = state.carry + text;
    const frames = [];
    let start = -1;
    let depth = 0;
    let inString = false;
    let escaped = false;
    for (let i = 0; i < buf.length; i++) {
        const c = buf[i];
        if (inString) {
            if (escaped) escaped = false;
            else if (c === '\\') escaped = true;
            else if (c === '"') inString = false;
        } else if (c === '"') {
            inString = true;
        } else if (c === '{' || c === '[') {
            if (depth === 0) start = i;
            depth++;
        } else if (c === '}' || c === ']') {
            depth--;
            if (depth === 0 && start > -1) {
                frames.push(buf.slice(start, i + 1));
                start = -1;
            }
        }
    }
    // Only an unfinished document is worth keeping. Anything before it has been
    // handed out already, and the rest is whitespace between frames.
    state.carry = depth > 0 && start > -1 ? buf.slice(start) : '';
    return frames;
}

export { BraidworkKikoSDK };
