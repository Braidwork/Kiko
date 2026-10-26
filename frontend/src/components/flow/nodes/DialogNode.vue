<script>
// import bootstrap from 'bootstrap/dist/js/bootstrap.js';
import { defineComponent, inject, watch, nextTick } from "vue";
// import { ElMessageBox } from 'element-plus'
import {
    copyProperties,
    getDefaultBranch,
    getRobotType,
    httpReq,
} from "../../../assets/tools.js";
import { useI18n } from "vue-i18n";
import EpPlus from "~icons/ep/plus";
import EpWarning from "~icons/ep/warning";
import EpChatDotRound from "~icons/ep/chat-dot-round";
import RiBold from "~icons/ri/bold";
import RiItalic from "~icons/ri/italic";
import RiStrikethrough from "~icons/ri/strikethrough";
import RiUnderline from "~icons/ri/underline";
import RiFontColor from "~icons/ri/font-color";
import BiHighlighter from "~icons/bi/highlighter";
import RiHeading from "~icons/ri/heading";
import RiListUnordered from "~icons/ri/list-unordered";
import RiListOrdered from "~icons/ri/list-ordered";
import RiChatQuoteLine from "~icons/ri/chat-quote-line";
import IcBaselineHorizontalRule from "~icons/ic/baseline-horizontal-rule";
import IcBaselineUndo from "~icons/ic/baseline-undo";
import IcBaselineRedo from "~icons/ic/baseline-redo";

// import {
//     // necessary extensions
//     Doc,
//     Text,
//     Paragraph,
//     //________________________
//     Color,
//     Heading,
//     FontSize,
//     Bold,
//     Underline,
//     Italic,
//     Strike,
//     BulletList,
//     OrderedList,
//     Link,
//     Table,
//     Image,
//     TaskList,
//     TextAlign,
//     FormatClear,
//     HorizontalRule,
// } from 'element-tiptap-vue3-fixed';
// import EleTipTap from './EleTipTap.vue'
// import Editor from '@tinymce/tinymce-vue'
import { Editor, EditorContent } from "@tiptap/vue-3";
import { BubbleMenu } from "@tiptap/vue-3/menus";
import StarterKit from "@tiptap/starter-kit";
import Underline from "@tiptap/extension-underline";
import { Color } from "@tiptap/extension-color";
import { FontFamily, TextStyle } from "@tiptap/extension-text-style";
import Highlight from "@tiptap/extension-highlight";
import Blockquote from "@tiptap/extension-blockquote";
import TextAlign from "@tiptap/extension-text-align";
import { Variable } from "./variableExtension.js";
import MaterialSymbolsFormatAlignLeft from "~icons/material-symbols/format-align-left";
import MaterialSymbolsFormatAlignCenter from "~icons/material-symbols/format-align-center";
import MaterialSymbolsFormatAlignRight from "~icons/material-symbols/format-align-right";
import PhTrash from "~icons/ph/trash";

export default defineComponent({
    name: "DialogNode",
    setup() {
        const { t, tm, rt } = useI18n();
        return { t, tm, rt };
    },
    inject: ["getGraph", "getNode"],
    data() {
        return {
            preview: "",
            nodeSetFormVisible: false,
            varDialogVisible: false,
            formLabelWidth: "100px",
            vars: [],
            selectedVar: "",
            overrideTimeoutEnabled: false,
            nodeData: {
                nodeName: this.t("dialogNode.nodeName"),
                dialogText: "",
                dialogTextType: "",
                dialogTextSource: "FixedText",
                dialogLlmGenPrompt: "",
                dialogFallbackText: "",
                branches: [],
                nextStep: "WaitUserResponse",
                responseStreaming: true,
                connectTimeout: -1,
                readTimeout: -1,
                contextLength: 5,
                valid: false,
                invalidMessages: [],
                newNode: true,
            },
            settings: {},
            nextSteps: [
                {
                    label: this.tm("dialogNode.nextSteps")[0],
                    value: "WaitUserResponse",
                },
                {
                    label: this.tm("dialogNode.nextSteps")[1],
                    value: "GotoNextNode",
                },
            ],
            loading: false,
            lastEditRange: null,
            // textEditor: '2',
            robotId: "",
            robotType: "",
            // extensions: [
            //     Color,
            //     Doc,
            //     Text,
            //     Paragraph,
            //     Heading.configure({ level: 5 }),
            //     FontSize,
            //     Bold.configure({ bubble: true }), // render command-button in bubble menu.
            //     Underline.configure({ bubble: true, menubar: false }), // render command-button in bubble menu but not in menubar.
            //     Italic,
            //     Strike,
            //     BulletList,
            //     OrderedList,
            //     Link,
            //     Table,
            //     Image,
            //     TaskList,
            //     TextAlign,
            //     FormatClear,
            //     HorizontalRule,
            // ],
            predefineColors: [
                "#ff4500",
                "#ff8c00",
                "#ffd700",
                "#90ee90",
                "#00ced1",
                "#1e90ff",
                "#c71585",
                // 'rgba(255, 69, 0, 0.68)',
                // 'rgb(255, 120, 0)',
                // 'hsv(51, 100, 98)',
                // 'hsva(120, 40, 94, 0.5)',
                // 'hsl(181, 100%, 37%)',
                // 'hsla(209, 100%, 56%, 0.73)',
                "#c7158577",
            ],
            editor: null,
            activeBtnColor: "#626aef",
            genTextVisible: false,
            genTextReq: {
                system: "",
                user: "",
            },
            textGenerating: false,
            genTextBtnText: "Generate text",
            generatedText: "",
        };
    },
    mounted() {
        // console.log('mounted dialognode')
        const node = this.getNode();
        const data = node.getData();
        // console.log(data);
        // console.log(this.nodeData);
        copyProperties(data, this.nodeData);
        this.setPreview();
        // console.log(this.nodeData);
        // if (data) {
        //     console.log(data);
        //     console.log(data.newNode);
        //     if (data.nodeName)
        //         this.nodeData.nodeName = data.nodeName;
        //     this.nodeData.dialogText = data.dialogText;
        //     if (data.nextStep)
        //         this.nodeData.nextStep = data.nextStep;
        //     if (data.newNode)
        //         this.nodeData.newNode = data.newNode;
        // }
        // console.log(node.getData().dialogText);
        const allNodeNameSet = inject("allNodeNameSet");
        if (this.nodeData.newNode) {
            let n = null;
            do {
                n =
                    n == null
                        ? Date.now().toString(16)
                        : Math.random().toString(16).substring(2);
                this.nodeData.nodeName =
                    this.t("dialogNode.nodeName") + "-" + n;
            } while (allNodeNameSet.value.has(this.nodeData.nodeName));
            const b = getDefaultBranch();
            b.branchType = "GotoAnotherNode";
            // b.conditionGroup[0][0].conditionType = 'UserInput';
            this.nodeData.branches.push(b);
            const heightOffset =
                this.$refs.nodeName.offsetHeight +
                this.$refs.nodeAnswer.offsetHeight +
                20;
            const x = this.$refs.nodeName.offsetWidth - 15;
            node.addPort({
                group: "absolute",
                args: { x: x, y: heightOffset },
                attrs: {
                    text: {
                        text: this.nextSteps[0].label,
                        fontSize: 12,
                    },
                },
            });
            this.nodeData.newNode = false;
            node.removeData({ silent: true });
            node.setData(this.nodeData, { silent: false });
        }
        allNodeNameSet.value.add(this.nodeData.nodeName);
        this.validate();
        node.on("change:data", ({ current }) => {
            // console.log(current);
            // const { name, text, nodeSetFormVisible } = current;
            this.nodeSetFormVisible = true;
            // delete current.dataInit;
        });
        nextTick(() => {
            this.setPortPos();
        });
        // this.nodeData.dialogText = '12345';
        const { robotId } = inject("robotId");
        this.robotId = robotId;
        this.robotType = getRobotType(this.robotId);
        console.log("robotType=" + this.robotType);
        // console.log(this.nodeData.dialogText)
        this.editor = new Editor({
            extensions: [
                // Blockquote,
                Color,
                Highlight.configure({ multicolor: true }),
                StarterKit,
                Underline,
                TextStyle,
                Variable,
                TextAlign.configure({
                    types: ["heading", "paragraph"],
                }),
            ],
            // content: '<p>I’m running Tiptap with Vue.js. 🎉</p>',
            content: this.nodeData.dialogText,
            editorProps: {
                // https://github.com/ueberdosis/tiptap/issues/943
                transformPastedText(text) {
                    return text
                        .replace(/​/g, "")
                        .replace(/[ 　]/g, " ");
                },
                transformPastedHTML(html) {
                    return html
                        .replace(/​/g, "")
                        .replace(/[ 　]/g, " ");
                },
            },
            onUpdate: () => {
                // HTML
                this.$emit("update:modelValue", this.editor.getHTML());
                this.nodeData.dialogText = this.editor.getHTML();

                // JSON
                // this.$emit('update:modelValue', this.editor.getJSON())
            },
        });
        this.$emit("update:modelValue", this.nodeData.dialogText);
        httpReq(
            "GET",
            "management/settings",
            { robotId: this.robotId },
            null,
            null,
        ).then((res) => {
            // const r = res.json();
            if (res.data) {
                copyProperties(res.data, this.settings);
                // updateBrief();
                // if (this.nodeData.connectTimeout > 0 && this.nodeData.readTimeout > 0)
                //     this.overrideTimeoutEnabled = true;
                if (this.nodeData.connectTimeout < 100)
                    this.nodeData.connectTimeout =
                        this.settings.chatProvider.connectTimeoutMillis;
                if (this.nodeData.readTimeout < 200)
                    this.nodeData.readTimeout =
                        this.settings.chatProvider.readTimeoutMillis;
            }
        });
    },
    beforeUnmount() {
        if (this.editor) this.editor.destroy();
    },
    unmounted() {
        // The watcher reads `editor` — guard against it firing after destroy.
        this.editor = null;
    },
    methods: {
        hideForm() {
            // const { nodeSetFormVisible } = this.getNode().getData();
            // console.log(this.getNode().getData());
            this.nodeSetFormVisible = false;
        },
        validate() {
            const d = this.nodeData;
            const m = d.invalidMessages;
            m.splice(0, m.length);
            if (!d.nodeName) m.push(this.tm("dialogNode.errors")[0]);
            if (d.dialogTextSource && d.dialogTextSource === "FixedText") {
                if (!d.dialogText || d.dialogText.length < 1)
                    m.push(this.tm("dialogNode.errors")[1]);
                else if (d.dialogText.length > 5000)
                    m.push(this.tm("dialogNode.errors")[2]);
            } else if (
                d.dialogTextSource &&
                d.dialogTextSource === "LlmGenText"
            ) {
                if (!d.dialogLlmGenPrompt || d.dialogLlmGenPrompt.length < 1)
                    m.push("Input prompt");
                else if (d.dialogLlmGenPrompt.length > 5000)
                    m.push("The prompt is too long.");
                if (!d.dialogFallbackText || d.dialogFallbackText.length < 1)
                    m.push("Input fallback text");
                else if (d.dialogFallbackText.length > 500)
                    m.push("The fallback text is too long");
            } else m.push("Unknown text source");
            d.valid = m.length == 0;
        },
        getTextWidth() {
            // let twts = document.getElementById('_twts');
            // if (!twts) {
            //     twts = document.createElement("span");
            //     twts.setAttribute("id", '_twts');
            //     twts.style.whiteSpace = 'no-wrap';
            //     twts.style.visibility = 'hidden';
            //     document.body.appendChild(twts);
            // }
            // twts.innerText = t;
            // return Math.ceil(twts.offsetWidth)
            // console.log(this.$refs.nodeAnswer.scrollWidth)
            return this.$refs.nodeAnswer.offsetWidth;
        },
        setPreview() {
            let t = "";
            if (
                this.nodeData.dialogTextSource &&
                this.nodeData.dialogTextSource === "FixedText"
            ) {
                t = this.nodeData.dialogText;
            } else if (
                this.nodeData.dialogTextSource &&
                this.nodeData.dialogTextSource === "LlmGenText"
            ) {
                t = this.nodeData.dialogLlmGenPrompt;
            }
            let p = this.dialogTextToPlain(t);
            if (p) {
                const array = p.split("\n");
                // console.log(array.splice(3, array.length - 3))
                if (array.length > 3) {
                    array.splice(3, array.length - 3, "......");
                }
                const node = this.getNode();
                // console.log(node.size().width)
                // console.log(this.getTextWidth())
                // console.log(this.$refs.nodeAnswer.getBoundingClientRect())
                array.forEach(function (item, idx, arr) {
                    if (
                        this.$refs.nodeAnswer.getBoundingClientRect().width >
                        node.size().width
                    ) {
                        const shortPercent =
                            (this.$refs.nodeAnswer.scrollWidth -
                                node.size().width) /
                            node.size().width;
                        // console.log(shortPercent)
                        // console.log(shortPercent * item.length)
                        console.log(Math.floor(shortPercent * item.length));
                        arr[idx] =
                            item.substring(
                                0,
                                Math.floor(shortPercent * item.length) - 5,
                            ) + "...";
                    }
                    // if (item.length > 20)
                    //     arr[idx] = item.substring(0, 20) + "...";
                }, this);
                p = array.join("\n");
                nextTick(() => {
                    this.setPortPos();
                });
            }
            this.preview = p;
        },
        setPortPos() {
            const node = this.getNode();
            const port = node.getPortAt(0);
            // console.log(this.$refs)
            const heightOffset =
                this.$refs.nodeName.offsetHeight +
                this.$refs.nodeAnswer.offsetHeight +
                20;
            // console.log(heightOffset);
            node.setPortProp(port.id, ["args", "y"], heightOffset);
            node.resize(node.size().width, 20 + heightOffset, {
                direction: "bottom",
            });
        },
        // Convert dialog text (plain or HTML with variable chips) to plain
        // preview text, keeping variable names wrapped in back ticks so the
        // template can render them as chips.
        dialogTextToPlain(t) {
            if (!t) return "";
            // Extract variable chip labels before stripping tags.
            const varsInText = [];
            const varHtml = t.replace(
                /<var\s+[^>]*data-var-name="([^"]*)"[^>]*>[\s\S]*?<\/var>/g,
                (m, name) => {
                    varsInText.push(name);
                    return "\x00" + (varsInText.length - 1) + "\x00";
                },
            );
            let p = varHtml
                .replace(/<br\s*\/?>/gi, "\n")
                .replace(/<\/p>/gi, "\n")
                .replace(/<[^>]+>/g, "")
                .replace(/\r/g, "");
            // Restore chip markers as `name` (back ticks are only a preview
            // marker here; backend still resolves them as real variables).
            p = p.replace(/\x00(\d+)\x00/g, (m, i) => "\x01" + varsInText[i] + "\x01");
            p = p.replace(/\x01([^\x01]*)\x01/g, "`$1`");
            return p;
        },
        // Split preview text into plain / variable segments for chip rendering.
        splitPreviewSegments(t) {
            const segments = [];
            if (!t) return segments;
            const parts = t.split(/`([^`]*)`/);
            for (let i = 0; i < parts.length; i++) {
                if (parts[i] === "") continue;
                segments.push({ isVar: i % 2 == 1, text: parts[i] });
            }
            return segments;
        },
        saveForm() {
            let text = "";
            for (let i = 0; i < this.nextSteps.length; i++) {
                if (this.nodeData.nextStep == this.nextSteps[i].value) {
                    text = this.nextSteps[i].label;
                    break;
                }
            }
            this.setPortPos();
            const node = this.getNode();
            const port = node.getPortAt(0);
            node.setPortProp(port.id, ["attrs", "text", "text"], text);
            // this.nodeData.dialogText = this.$refs.textArea.innerText;
            const branch = this.nodeData.branches[0];
            branch.branchName = text;
            branch.branchId = port.id;
            this.validate();
            this.setPreview();
            // All robot types now edit through tiptap; the editor emits HTML.
            // Callers that only accept plain text read `preview` instead.
            this.nodeData.dialogTextType = "TextHtml";
            // console.log('dialogTextType=' + this.nodeData.dialogTextType);
            // console.log(this.preview);
            node.removeData({ silent: true });
            node.setData(this.nodeData, { silent: false });
            this.hideForm();
            // console.log(node.getData());
        },
        getSel() {
            const sel = window.getSelection();
            // console.log(sel, sel.rangeCount, 'sel')
            if (sel.rangeCount > 0) {
                this.lastEditRange = sel.getRangeAt(0);
                // console.log(this.lastEditRange);
            }
        },
        async showVarsForm() {
            let t = await httpReq(
                "GET",
                "variable",
                { robotId: this.robotId },
                null,
                null,
            );
            // console.log(t);
            if (t && t.status == 200 && t.data) {
                this.vars = t.data;
            }
            this.varDialogVisible = true;
        },
        insertVar() {
            if (!this.selectedVar) return;
            const item = this.vars.find(
                (v) => v.varName === this.selectedVar,
            );
            if (this.editor) {
                // Rich-text editor: insert an atomic variable chip at the cursor.
                this.editor
                    .chain()
                    .focus()
                    .insertVariable(this.selectedVar, item?.varType)
                    .run();
                this.nodeData.dialogText = this.editor.getHTML();
                this.$emit("update:modelValue", this.nodeData.dialogText);
            } else {
                this.nodeData.dialogText += "`" + this.selectedVar + "`";
            }
            this.varDialogVisible = false;
        },
        changeEditorNote() {
            // console.log('this.nodeData.dialogText=' + this.nodeData.dialogText)
            // console.log('this.textEditor='+this.textEditor)
            if (this.nodeData.dialogText) {
                if (this.textEditor == "1") {
                    ElMessageBox.confirm(
                        "Switch to plain text and all styles will be lost. Whether to continue?",
                        "Warning",
                        {
                            confirmButtonText: "OK",
                            cancelButtonText: "Cancel",
                            type: "warning",
                        },
                    )
                        .then(() => {
                            const c = this.nodeData.dialogText
                                .replace(/<\/p>/g, "\n")
                                .trim();
                            const regex = /(<([^>]+)>)/gi;
                            this.nodeData.dialogText = c.replace(regex, "");
                        })
                        .catch(() => {
                            this.textEditor = "2";
                        });
                } else {
                    const c =
                        "<p>" +
                        this.nodeData.dialogText.replace(/\n/g, "</p><p>") +
                        "</p>";
                    // console.log(c);
                    this.$refs.editor.setContent(c);
                }
            }
        },
        // editorCallback(s) {
        //     console.log('callback=' + s);
        // },
        async genText() {
            const prompt = [{ role: "user", content: this.genTextReq.user }];
            if (this.genTextReq.system)
                prompt.push({
                    role: "system",
                    content: this.genTextReq.system,
                });
            // const prompt = [];
            // if (this.genTextReq.system)
            //     prompt.push("<|system|>", this.genTextReq.system);
            // prompt.push('<|user|>', this.genTextReq.user + '</s>', '<|assistant|>');
            const body = {
                robot_id: this.robotId,
                prompt: JSON.stringify(prompt),
                // prompt: prompt.join('\n')
            };
            // const endPos = this.editor.state.doc.content.size;
            // this.editor.commands.insertContentAt(endPos, '00000000000000000abc123')
            // console.log(this.nodeData.dialogText)
            // console.log(JSON.stringify(body));
            this.textGenerating = true;
            this.genTextBtnText = "Generating";
            this.generatedText = "";
            const u =
                import.meta.env.VITE_REQ_BACKEND_PREFIX + "ai/text/generation";
            // const u = 'http://localhost:12715/ai/text/generation';
            const response = await fetch(u, {
                method: "POST",
                headers: {
                    "Content-Type": "text/event-stream",
                },
                body: JSON.stringify(body),
            });
            const reader = response.body
                .pipeThrough(new TextDecoderStream())
                .getReader();
            while (true) {
                const { value, done } = await reader.read();
                console.log("Received", value);
                this.processGenedText(value);
                if (done) {
                    console.log("Connection finished.");
                    this.textGenerating = false;
                    this.genTextBtnText = "Generate text";
                    this.genTextVisible = false;
                    if (this.editor) {
                        const endPos = this.editor.state.doc.content.size;
                        this.editor.commands.insertContentAt(
                            endPos,
                            this.generatedText,
                        );
                    } else {
                        this.nodeData.dialogText += this.generatedText;
                    }
                    break;
                }
            }
        },
        processGenedText(t) {
            if (t == null || t == undefined) return;
            const chunks = t.split("\n\n");
            let s = "";
            chunks.forEach((chunk, idx, array) => {
                let dataList = chunk.split("\n");
                dataList.forEach((data, i, a) => {
                    if (data.indexOf("data: ") == 0) {
                        let d = data.substring(6);
                        if (d.length > 0) s += d;
                    }
                });
            });
            if (s.length > 0) this.generatedText += s;
            // console.log(this.nodeData.dialogText)
        },
        showColorPicker() {
            // this.$refs.colorPicker.show()
            this.$refs.colorPicker.click();
        },
    },
    components: {
        EpPlus,
        EpWarning,
        EpChatDotRound,
        RiBold,
        RiItalic,
        RiStrikethrough,
        RiUnderline,
        MaterialSymbolsFormatAlignLeft,
        MaterialSymbolsFormatAlignCenter,
        MaterialSymbolsFormatAlignRight,
        BiHighlighter,
        RiFontColor,
        RiHeading,
        RiListUnordered,
        RiListOrdered,
        RiChatQuoteLine,
        IcBaselineHorizontalRule,
        PhTrash,
        IcBaselineUndo,
        IcBaselineRedo,
        // EleTipTap,
        EditorContent,
        BubbleMenu,
        // 'editor': Editor
    },
    props: {
        modelValue: {
            type: String,
            default: "",
        },
    },

    emits: ["update:modelValue"],
    watch: {
        modelValue(value) {
            if (!this.editor) return;
            // HTML
            const isSame = this.editor.getHTML() === value;

            // JSON
            // const isSame = JSON.stringify(this.editor.getJSON()) === JSON.stringify(value)

            if (isSame) {
                return;
            }

            this.editor.commands.setContent(value, false);
        },
    },
});
/*
watch(this.nodeData.dialogText, async (newT, oldT) => {
    if (newT != oldT) {
        this.preview = newT.replace(/<[^>]+>/g, '');
    }
    // if (newQuestion.indexOf('?') > -1) {
    //     answer.value = 'Thinking...'
    //     try {
    //         const res = await fetch('https://yesno.wtf/api')
    //         answer.value = (await res.json()).answer
    //     } catch (error) {
    //         answer.value = 'Error! Could not reach the API. ' + error
    //     }
    // }
})
*/
</script>
<style scoped>
.nodeBox {
    border: 1px solid #e2e8f0;
    border-radius: 10px;
    height: 100%;
    width: 100%;
    background-color: #fff;
    overflow: hidden;
    box-shadow: 0 2px 8px rgba(31, 45, 61, 0.08);
}

.nodeTitle {
    display: flex;
    align-items: center;
    gap: 6px;
    background: linear-gradient(135deg, #ffc400, #ff9500);
    color: #fff;
    font-weight: 600;
    font-size: 13px;
    padding: 7px 10px;
}

.titleIcon {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 18px;
    border-radius: 5px;
    background: rgba(255, 255, 255, 0.2);
}

.titleText {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
    min-width: 0;
}

.nodeBody {
    padding: 8px 10px;
    white-space: pre-wrap;
    font-size: 12px;
    line-height: 1.6;
    color: #4e5969;
    overflow: hidden;
}

/* Variable chip on the canvas node preview */
.previewVarChip {
    display: inline-block;
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    font-size: 11px;
    line-height: 1.5;
    padding: 0 5px;
    margin: 0 1px;
    border-radius: 6px;
    background: #eef2ff;
    border: 1px solid #c7d2fe;
    color: #4f46e5;
    white-space: nowrap;
    vertical-align: baseline;
}

/* .optionWidth {
    width: 110px;
} */

/* .divInputBox {
    height: 100px;
    width: 100%;
    padding: 5px;
    overflow-y: auto;
    border: 1px solid #000;
    line-height: 150%;
} */
/* box-shadow: 0 0 5px 5px rgba(0, 0, 0, 0.28); */
/* #bubbleMenu button,
.menubar button {
    border-left: 1px solid black;
    border-top: 1px solid black;
    border-bottom: 1px solid black;
    border-right: none;
}

#bubbleMenu button:last-child,
.menubar button:last-child {
    border: 1px solid black;
}

.is-active {
    color: white;
    background-color: black;
} */
</style>
<style lang="scss">
/* Basic editor styles */
.tiptap {
    outline: none;

    :first-child {
        margin-top: 0;
    }

    :last-child {
        margin-bottom: 0;
    }

    p {
        margin: 0.35em 0;
        line-height: 1.7;
    }

    blockquote {
        border-left: 3px solid #c9cdd4;
        margin: 0.5em 0;
        padding-left: 1rem;
        color: #6b7280;
    }

    mark {
        background-color: #faf594;
        border-radius: 0.4rem;
        box-decoration-break: clone;
        padding: 0.1rem 0.3rem;
    }

    /* Variable chip rendered by the custom `variable` inline node */
    var.var-chip {
        display: inline-block;
        font-style: normal;
        font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
        font-size: 0.86em;
        line-height: 1.6;
        padding: 0 0.5em;
        margin: 0 0.15em;
        border-radius: 0.45em;
        background: #eef2ff;
        border: 1px solid #c7d2fe;
        color: #4f46e5;
        white-space: nowrap;
        user-select: none;
        -webkit-user-select: none;

        &.ProseMirror-selectednode {
            background: #e0e7ff;
            box-shadow: 0 0 0 2px rgba(99, 102, 241, 0.35);
        }
    }
}

/* Rich text editor container */
.editorWrap {
    width: 100%;

    .ProseMirror {
        min-height: 160px;
        max-height: 420px;
        overflow-y: auto;
        padding: 12px 14px;
        background: #fff;
        border: 1px solid #e2e8f0;
        border-radius: 8px;
        font-size: 14px;
        color: var(--app-text);
        transition: border-color 0.2s, box-shadow 0.2s;

        &:hover {
            border-color: #c7cbf7;
        }

        &:focus {
            border-color: var(--app-primary);
            box-shadow: 0 0 0 2px rgba(99, 102, 241, 0.12);
        }
    }
}
</style>
<template>
    <div class="nodeBox">
        <div ref="nodeName" class="nodeTitle">
            <span class="titleIcon">
                <el-icon size="12">
                    <EpChatDotRound />
                </el-icon>
            </span>
            <span class="titleText">{{ nodeData.nodeName }}</span>
            <span v-show="nodeData.invalidMessages.length > 0">
                <el-tooltip
                    class="box-item"
                    effect="dark"
                    :content="nodeData.invalidMessages.join('<br/>')"
                    placement="bottom"
                    raw-content
                >
                    <el-icon color="red" size="16">
                        <EpWarning />
                    </el-icon>
                </el-tooltip>
            </span>
        </div>
        <div ref="nodeAnswer" class="nodeBody">
            <template
                v-for="(seg, idx) in splitPreviewSegments(preview)"
                :key="idx"
            >
                <span v-if="seg.isVar" class="previewVarChip">{{
                    seg.text
                }}</span>
                <template v-else>{{ seg.text }}</template>
            </template>
        </div>
        <!-- <el-text ref="nodeAnswer" line-clamp="2">
            {{ preview }}
        </el-text> -->
        <!-- <el-text truncated ref="nodeAnswer">{{ nodeData.dialogText }}</el-text> -->
        <!-- <teleport to="#modal-container"> -->
        <!-- <teleport to="body"> -->
        <el-drawer
            v-model="nodeSetFormVisible"
            :title="nodeData.nodeName"
            direction="rtl"
            size="72%"
            :append-to-body="true"
            :destroy-on-close="true"
        >
            <el-form :model="nodeData">
                <el-form-item
                    :label="t('common.nodeName')"
                    :label-width="formLabelWidth"
                    prop="nodeName"
                    :rules="[
                        { required: true, message: 'nodeName is required' },
                    ]"
                >
                    <el-input v-model="nodeData.nodeName" autocomplete="off" />
                </el-form-item>
                <el-form-item label="" :label-width="formLabelWidth">
                    <!-- <el-switch v-model="nodeData.textFromLLM" class="mb-2" active-text="LLM" inactive-text="Fixed text"
                        style="--el-switch-off-color: #13ce66" /> -->
                    <el-radio-group v-model="nodeData.dialogTextSource">
                        <el-radio value="FixedText">{{
                            tm("dialogNode.textSource")[0]
                        }}</el-radio>
                        <el-radio value="LlmGenText">{{
                            tm("dialogNode.textSource")[1]
                        }}</el-radio>
                    </el-radio-group>
                </el-form-item>
                <!-- <el-tabs v-model="activeName" class="demo-tabs" @tab-click="handleClick">
                    <el-tab-pane label="Fixed text" name="first">User</el-tab-pane>
                    <el-tab-pane label="Large model generates text" name="second">Config</el-tab-pane>
                </el-tabs> -->
                <el-form-item
                    v-show="nodeData.dialogTextSource == 'FixedText'"
                    :label="t('dialogNode.form.label')"
                    :label-width="formLabelWidth"
                >
                    <!-- <el-radio-group v-model="textEditor" class="ml-4" @change="changeEditorNote">
                            <el-radio value="1">Plain text</el-radio>
                            <el-radio value="2">Rich text</el-radio>
                        </el-radio-group> -->
                    <!-- <div v-show="textEditor == '1'" ref="textArea" v-text="nodeData.dialogText" class="divInputBox"
                            contenteditable="true" @blur="getSel"></div> -->
                    <!-- <EleTipTap v-show="textEditor == '2'" :editorText="nodeData.dialogText" @updatedEditorText="editorCallback" /> -->
                    <!-- Current using follow one -->
                    <!-- <el-tiptap ref="editor" v-show="textEditor == '2'" v-model:content="nodeData.dialogText"
                            :extensions="extensions" /> -->
                    <bubble-menu
                        id="bubbleMenu"
                        :editor="editor"
                        :tippy-options="{ duration: 100 }"
                        v-if="editor && robotType == 'TextBot'"
                    >
                        <button
                            type="button"
                            class="inactive"
                            @click="editor.chain().focus().toggleBold().run()"
                            :class="{ 'is-active': editor.isActive('bold') }"
                        >
                            bold
                        </button>
                        <button
                            type="button"
                            class="inactive"
                            @click="editor.chain().focus().toggleItalic().run()"
                            :class="{ 'is-active': editor.isActive('italic') }"
                        >
                            italic
                        </button>
                        <button
                            type="button"
                            class="inactive"
                            @click="editor.chain().focus().toggleStrike().run()"
                            :class="{ 'is-active': editor.isActive('strike') }"
                        >
                            strike
                        </button>
                    </bubble-menu>
                    <div
                        class="menubar"
                        v-if="editor && robotType == 'TextBot'"
                    >
                        <el-button-group size="default">
                            <el-button
                                :color="
                                    editor.isActive('bold')
                                        ? activeBtnColor
                                        : ''
                                "
                                @click="
                                    editor.chain().focus().toggleBold().run()
                                "
                                ><el-icon> <RiBold /> </el-icon
                            ></el-button>
                            <el-button
                                :color="
                                    editor.isActive('italic')
                                        ? activeBtnColor
                                        : ''
                                "
                                @click="
                                    editor.chain().focus().toggleItalic().run()
                                "
                                ><el-icon> <RiItalic /> </el-icon
                            ></el-button>
                            <el-button
                                :color="
                                    editor.isActive('strike')
                                        ? activeBtnColor
                                        : ''
                                "
                                @click="
                                    editor.chain().focus().toggleStrike().run()
                                "
                                ><el-icon> <RiStrikethrough /> </el-icon
                            ></el-button>
                            <el-button
                                :color="
                                    editor.isActive('underline')
                                        ? activeBtnColor
                                        : ''
                                "
                                @click="
                                    editor
                                        .chain()
                                        .focus()
                                        .toggleUnderline()
                                        .run()
                                "
                                ><el-icon> <RiUnderline /> </el-icon
                            ></el-button>
                            <el-button
                                :color="
                                    editor.isActive({ textAlign: 'left' })
                                        ? activeBtnColor
                                        : ''
                                "
                                @click="
                                    editor
                                        .chain()
                                        .focus()
                                        .setTextAlign('left')
                                        .run()
                                "
                                ><el-icon>
                                    <MaterialSymbolsFormatAlignLeft /> </el-icon
                            ></el-button>
                            <el-button
                                :color="
                                    editor.isActive({ textAlign: 'center' })
                                        ? activeBtnColor
                                        : ''
                                "
                                @click="
                                    editor
                                        .chain()
                                        .focus()
                                        .setTextAlign('center')
                                        .run()
                                "
                                ><el-icon>
                                    <MaterialSymbolsFormatAlignCenter /> </el-icon
                            ></el-button>
                            <el-button
                                :color="
                                    editor.isActive({ textAlign: 'right' })
                                        ? activeBtnColor
                                        : ''
                                "
                                @click="
                                    editor
                                        .chain()
                                        .focus()
                                        .setTextAlign('right')
                                        .run()
                                "
                                ><el-icon>
                                    <MaterialSymbolsFormatAlignRight /> </el-icon
                            ></el-button>
                            <el-button
                                :color="
                                    editor.isActive('highlight')
                                        ? activeBtnColor
                                        : ''
                                "
                                @click="
                                    editor
                                        .chain()
                                        .focus()
                                        .toggleHighlight()
                                        .run()
                                "
                                ><el-icon> <BiHighlighter /> </el-icon
                            ></el-button>
                            <el-button
                                :color="editor.getAttributes('textStyle').color"
                                @click="showColorPicker"
                                ><el-icon> <RiFontColor /> </el-icon
                            ></el-button>
                            <el-button
                                :color="
                                    editor.isActive('heading', { level: 1 })
                                        ? activeBtnColor
                                        : ''
                                "
                                @click="
                                    editor
                                        .chain()
                                        .focus()
                                        .toggleHeading({ level: 1 })
                                        .run()
                                "
                                ><el-icon> <RiHeading /> </el-icon
                            ></el-button>
                            <el-button
                                :color="
                                    editor.isActive('bulletList')
                                        ? activeBtnColor
                                        : ''
                                "
                                @click="
                                    editor
                                        .chain()
                                        .focus()
                                        .toggleBulletList()
                                        .run()
                                "
                                ><el-icon> <RiListUnordered /> </el-icon
                            ></el-button>
                            <el-button
                                :color="
                                    editor.isActive('orderedList')
                                        ? activeBtnColor
                                        : ''
                                "
                                @click="
                                    editor
                                        .chain()
                                        .focus()
                                        .toggleOrderedList()
                                        .run()
                                "
                                ><el-icon> <RiListOrdered /> </el-icon
                            ></el-button>
                            <el-button
                                :color="
                                    editor.isActive('blockquote')
                                        ? activeBtnColor
                                        : ''
                                "
                                @click="
                                    editor
                                        .chain()
                                        .focus()
                                        .toggleBlockquote()
                                        .run()
                                "
                                ><el-icon> <RiChatQuoteLine /> </el-icon
                            ></el-button>
                            <el-button
                                @click="
                                    editor
                                        .chain()
                                        .focus()
                                        .setHorizontalRule()
                                        .run()
                                "
                                ><el-icon>
                                    <IcBaselineHorizontalRule /> </el-icon
                            ></el-button>
                            <el-button
                                @click="
                                    editor
                                        .chain()
                                        .focus()
                                        .clearNodes()
                                        .unsetAllMarks()
                                        .run()
                                "
                                ><el-icon> <PhTrash /> </el-icon
                            ></el-button>
                            <el-button
                                @click="editor.chain().focus().undo().run()"
                                ><el-icon> <IcBaselineUndo /> </el-icon
                            ></el-button>
                            <el-button
                                @click="editor.chain().focus().redo().run()"
                                ><el-icon> <IcBaselineRedo /> </el-icon
                            ></el-button>
                        </el-button-group>
                        <input
                            ref="colorPicker"
                            type="color"
                            style="visibility: hidden"
                            @input="
                                (e) =>
                                    editor
                                        .chain()
                                        .focus()
                                        .setColor(e.target.value)
                                        .run()
                            "
                        />
                        <!-- <el-color-picker ref="colorPicker" :predefine="predefineColors"
                            v-model="editor.getAttributes('textStyle').color"
                            @change="(v) => editor.chain().focus().setColor(v).run()" style="visibility: hidden;" /> -->
                        <!-- <input type="color" @input="editor.chain().focus().setColor($event.target.value).run()"
                            :value="editor.getAttributes('textStyle').color"> -->
                        <!-- <button type="button" :class="{ 'is-active': editor.isActive('code') }"
                            @click="editor.chain().focus().toggleCode().run()">
                            Code
                        </button>
                        <button type="button" :class="{ 'is-active': editor.isActive('codeBlock') }"
                            @click="editor.chain().focus().toggleCodeBlock().run()">
                            CodeBlock
                        </button> -->
                    </div>
                    <editor-content
                        class="editorWrap"
                        :editor="editor"
                        v-if="editor"
                    />
                </el-form-item>
                <el-form-item
                    v-show="nodeData.dialogTextSource == 'FixedText'"
                    label=""
                    :label-width="formLabelWidth"
                >
                    <el-button @click="showVarsForm">
                        <el-icon>
                            <EpPlus />
                        </el-icon>
                        {{ t("dialogNode.form.addVar") }}
                    </el-button>
                    <el-button @click="genTextVisible = true">
                        {{ tm("dialogNode.btns")[0] }}
                    </el-button>
                </el-form-item>
                <el-form-item
                    v-show="nodeData.dialogTextSource == 'LlmGenText'"
                    label="Prompt"
                    :label-width="formLabelWidth"
                >
                    <el-input
                        v-model="nodeData.dialogLlmGenPrompt"
                        :rows="5"
                        type="textarea"
                        placeholder="Tell the large model how to respond"
                    />
                </el-form-item>
                <el-form-item
                    v-show="nodeData.dialogTextSource == 'LlmGenText'"
                    label="Fallback text"
                    :label-width="formLabelWidth"
                >
                    <el-input
                        v-model="nodeData.dialogFallbackText"
                        :rows="2"
                        type="textarea"
                        placeholder="What should be replied when the call times out?"
                    />
                </el-form-item>
                <el-form-item
                    v-show="nodeData.dialogTextSource == 'LlmGenText'"
                    label="Timeout"
                    :label-width="formLabelWidth"
                >
                    <el-checkbox
                        label="Override timeout of settings"
                        v-model="overrideTimeoutEnabled"
                    />
                    <el-divider direction="vertical" />
                    Global settings are: [Connect:
                    {{ settings.chatProvider?.connectTimeoutMillis }} millis,
                    Read: {{ settings.chatProvider?.readTimeoutMillis }} mills]
                </el-form-item>
                <el-form-item
                    v-show="nodeData.dialogTextSource == 'LlmGenText'"
                    label="Connection"
                    :label-width="formLabelWidth"
                >
                    <el-input-number
                        v-model="nodeData.connectTimeout"
                        :min="100"
                        :max="65500"
                        :disabled="!overrideTimeoutEnabled"
                    />
                </el-form-item>
                <el-form-item
                    v-show="nodeData.dialogTextSource == 'LlmGenText'"
                    label="Read"
                    :label-width="formLabelWidth"
                >
                    <el-input-number
                        v-model="nodeData.readTimeout"
                        :min="200"
                        :max="65500"
                        :disabled="!overrideTimeoutEnabled"
                    />
                </el-form-item>
                <el-form-item
                    v-show="nodeData.dialogTextSource == 'LlmGenText'"
                    label="History"
                    :label-width="formLabelWidth"
                >
                    <el-input-number
                        v-model="nodeData.contextLength"
                        :min="0"
                        :max="50"
                        :step="5"
                    />
                    How many chat history records will be added.
                </el-form-item>
                <el-form-item
                    v-show="nodeData.dialogTextSource == 'LlmGenText'"
                    label="Streaming"
                    :label-width="formLabelWidth"
                >
                    <el-checkbox
                        v-model="nodeData.responseStreaming"
                        label="Response streaming"
                    />
                </el-form-item>
                <el-form-item
                    :label="t('dialogNode.form.nextStep')"
                    :label-width="formLabelWidth"
                >
                    <el-select
                        v-model="nodeData.nextStep"
                        :placeholder="t('dialogNode.form.choose')"
                    >
                        <el-option
                            v-for="item in nextSteps"
                            :key="item.label"
                            :label="item.label"
                            :value="item.value"
                        />
                    </el-select>
                </el-form-item>
            </el-form>
            <div class="demo-drawer__footer">
                <el-button
                    type="primary"
                    :loading="loading"
                    @click="saveForm()"
                    >{{ t("common.save") }}</el-button
                >
                <el-button @click="hideForm()">{{
                    t("common.cancel")
                }}</el-button>
            </div>
        </el-drawer>
        <el-dialog
            v-model="varDialogVisible"
            :title="t('dialogNode.var.title')"
            width="30%"
            :append-to-body="true"
            :destroy-on-close="true"
        >
            <el-select
                v-model="selectedVar"
                class="m-2"
                :placeholder="t('dialogNode.var.choose')"
                size="large"
            >
                <el-option
                    v-for="item in vars"
                    :key="item.varName"
                    :label="item.varName"
                    :value="item.varName"
                />
            </el-select>
            <template #footer>
                <span class="dialog-footer">
                    <el-button type="primary" @click="insertVar">
                        {{ t("common.insert") }}
                    </el-button>
                    <el-button @click="varDialogVisible = false">{{
                        t("common.cancel")
                    }}</el-button>
                </span>
            </template>
        </el-dialog>
        <el-dialog
            v-model="genTextVisible"
            title="Prompt"
            width="70%"
            :append-to-body="true"
            :destroy-on-close="true"
        >
            <el-form :model="genTextReq">
                <el-form-item label="System" :label-width="formLabelWidth">
                    <el-input v-model="genTextReq.system" autocomplete="on" />
                </el-form-item>
                <el-form-item label="User *" :label-width="formLabelWidth">
                    <el-input
                        v-model="genTextReq.user"
                        autocomplete="on"
                        :row="5"
                        type="textarea"
                    />
                </el-form-item>
                <el-form-item
                    v-if="generatedText.length > 0"
                    label=""
                    :label-width="formLabelWidth"
                >
                    Following generated text will be inserted when finish.
                </el-form-item>
                <el-form-item label="" :label-width="formLabelWidth">
                    {{ generatedText }}
                </el-form-item>
            </el-form>
            <template #footer>
                <span class="dialog-footer">
                    <el-button
                        type="primary"
                        @click="genText"
                        :loading="textGenerating"
                    >
                        {{ genTextBtnText }}
                    </el-button>
                    <el-button @click="genTextVisible = false">{{
                        t("common.cancel")
                    }}</el-button>
                </span>
            </template>
        </el-dialog>
        <!-- </teleport> -->
    </div>
</template>
