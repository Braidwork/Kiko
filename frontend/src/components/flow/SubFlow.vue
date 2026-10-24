<script setup>
import {
    h,
    ref,
    onMounted,
    onUnmounted,
    nextTick,
    provide,
    readonly,
} from "vue";
import { useRoute, useRouter } from "vue-router";
import CollectNode from "./nodes/CollectNode.vue";
import ConditionNode from "./nodes/ConditionNode.vue";
import DialogNode from "./nodes/DialogNode.vue";
import KnowledgeBaseAnswerNode from "./nodes/KnowledgeBaseAnswerNode.vue";
import EndNode from "./nodes/EndNode.vue";
import GotoNode from "./nodes/GotoNode.vue";
import ExternalHttpNode from "./nodes/ExternalHttpNode.vue";
import SendEmailNode from "./nodes/SendEmailNode.vue";
import LlmChatNode from "./nodes/LlmChatNode.vue";
import { Graph } from "@antv/x6";
// https://x6.antv.vision/zh/docs/tutorial/advanced/react#%E6%B8%B2%E6%9F%93-vue-%E8%8A%82%E7%82%B9
import { register, getTeleport } from "@antv/x6-vue-shape";
import { atob, httpReq } from "../../assets/tools.js";
import { BraidworkKikoSDK } from "../../assets/BraidworkKikoSDK.js";
// import { ElNotification, ElMessage, ElMessageBox } from 'element-plus';
import { useI18n } from "vue-i18n";
import EpDelete from "~icons/ep/delete";
import EpEdit from "~icons/ep/edit";
import EpFinished from "~icons/ep/finished";
import EpPlus from "~icons/ep/plus";
import EpPromotion from "~icons/ep/promotion";
import EpDArrowRight from "~icons/ep/d-arrow-right";
import EpChatDotRound from "~icons/ep/chat-dot-round";
import EpSwitch from "~icons/ep/switch";
import EpCollection from "~icons/ep/collection";
import EpReading from "~icons/ep/reading";
import EpMagicStick from "~icons/ep/magic-stick";
import EpConnection from "~icons/ep/connection";
import EpMessage from "~icons/ep/message";
import EpCircleClose from "~icons/ep/circle-close";
const { t, tm, rt } = useI18n();

const route = useRoute();
const router = useRouter();
const robotId = route.params.robotId;
// console.log(router.currentRoute.from)
const TeleportContainer = getTeleport();

const subFlows = ref([]);
const subflowNames = ref([]);
const allNodeNameSet = ref(new Set());

function updateSubFlowNames() {
    // console.log(subFlows);
    const names = new Array();
    for (let i = 0; i < subFlows.value.length; i++) {
        // console.log(subFlows.value[i].id);
        names.push({ id: subFlows.value[i].id, name: subFlows.value[i].name });
    }
    // console.log(names);
    return names;
}

// provide('getSubFlowNames', {readonly(subflowNames), updateSubFlowNames})
provide("subFlowNamesFn", { subflowNames, updateSubFlowNames });
provide("robotId", { robotId });
provide("allNodeNameSet", allNodeNameSet);

register({
    shape: "CollectNode",
    width: 270,
    height: 120,
    component: CollectNode,
    ports: {
        groups: {
            absolute: {
                position: {
                    name: "absolute",
                },
                attrs: {
                    circle: {
                        r: 5,
                        magnet: true,
                        stroke: "#a2a9b8",
                        strokeWidth: 1,
                        fill: "#fff",
                        style: {
                            visibility: "show",
                        },
                    },
                },
                label: {
                    position: "left",
                },
            },
        },
    },
});

register({
    shape: "ConditionNode",
    width: 270,
    height: 100,
    component: ConditionNode,
    ports: {
        groups: {
            absolute: {
                position: {
                    name: "absolute",
                },
                attrs: {
                    circle: {
                        r: 5,
                        magnet: true,
                        stroke: "#a2a9b8",
                        strokeWidth: 1,
                        fill: "#fff",
                        style: {
                            visibility: "show",
                        },
                    },
                },
                label: {
                    position: "left",
                },
            },
        },
    },
});

register({
    shape: "DialogNode",
    width: 270,
    height: 100,
    component: DialogNode,
    ports: {
        groups: {
            absolute: {
                position: {
                    name: "absolute",
                },
                attrs: {
                    circle: {
                        r: 5,
                        magnet: true,
                        stroke: "#a2a9b8",
                        strokeWidth: 1,
                        fill: "#fff",
                        style: {
                            visibility: "show",
                        },
                    },
                },
                label: {
                    position: "left",
                },
            },
        },
    },
});

register({
    shape: "KnowledgeBaseAnswerNode",
    width: 270,
    height: 150,
    component: KnowledgeBaseAnswerNode,
    ports: {
        groups: {
            absolute: {
                position: {
                    name: "absolute",
                },
                attrs: {
                    circle: {
                        r: 5,
                        magnet: true,
                        stroke: "#a2a9b8",
                        strokeWidth: 1,
                        fill: "#fff",
                        style: {
                            visibility: "show",
                        },
                    },
                },
                label: {
                    position: "left",
                },
            },
        },
    },
});

register({
    shape: "GotoNode",
    width: 270,
    height: 100,
    component: GotoNode,
    ports: {
        groups: {
            absolute: {
                position: {
                    name: "absolute",
                },
                attrs: {
                    circle: {
                        r: 5,
                        magnet: true,
                        stroke: "#a2a9b8",
                        strokeWidth: 1,
                        fill: "#fff",
                        style: {
                            visibility: "show",
                        },
                    },
                },
                label: {
                    position: "left",
                },
            },
        },
    },
});

register({
    shape: "ExternalHttpNode",
    width: 270,
    height: 100,
    component: ExternalHttpNode,
    ports: {
        groups: {
            absolute: {
                position: {
                    name: "absolute",
                },
                attrs: {
                    circle: {
                        r: 5,
                        magnet: true,
                        stroke: "#a2a9b8",
                        strokeWidth: 1,
                        fill: "#fff",
                        style: {
                            visibility: "show",
                        },
                    },
                },
                label: {
                    position: "left",
                },
            },
        },
    },
});

register({
    shape: "SendEmailNode",
    width: 270,
    height: 100,
    component: SendEmailNode,
    ports: {
        groups: {
            absolute: {
                position: {
                    name: "absolute",
                },
                attrs: {
                    circle: {
                        r: 5,
                        magnet: true,
                        stroke: "#a2a9b8",
                        strokeWidth: 1,
                        fill: "#fff",
                        style: {
                            visibility: "show",
                        },
                    },
                },
                label: {
                    position: "left",
                },
            },
        },
    },
});

register({
    shape: "EndNode",
    width: 270,
    height: 100,
    component: EndNode,
    ports: {
        groups: {
            absolute: {
                position: {
                    name: "absolute",
                },
                attrs: {
                    circle: {
                        r: 5,
                        magnet: true,
                        stroke: "#a2a9b8",
                        strokeWidth: 1,
                        fill: "#fff",
                        style: {
                            visibility: "show",
                        },
                    },
                },
                label: {
                    position: "left",
                },
            },
        },
    },
});

register({
    shape: "LlmChatNode",
    width: 270,
    height: 120,
    component: LlmChatNode,
    ports: {
        groups: {
            absolute: {
                position: {
                    name: "absolute",
                },
                attrs: {
                    circle: {
                        r: 5,
                        magnet: true,
                        stroke: "#a2a9b8",
                        strokeWidth: 1,
                        fill: "#fff",
                        style: {
                            visibility: "show",
                        },
                    },
                },
                label: {
                    position: "left",
                },
            },
        },
    },
});

const nodes = [
    {
        name: tm("flow.nodes")[0],
        type: "DialogNode",
        desc: tm("flow.nodesDesc")[0],
        icon: EpChatDotRound,
    },
    {
        name: tm("flow.nodes")[4],
        type: "KnowledgeBaseAnswerNode",
        desc: "Knowledge base answer node",
        icon: EpReading,
    },
    {
        name: tm("flow.nodes")[5],
        type: "LlmChatNode",
        desc: "Llm chat node",
        icon: EpMagicStick,
    },
    {
        name: tm("flow.nodes")[1],
        type: "ConditionNode",
        desc: tm("flow.nodesDesc")[1],
        icon: EpSwitch,
    },
    {
        name: tm("flow.nodes")[2],
        type: "CollectNode",
        desc: tm("flow.nodesDesc")[2],
        icon: EpCollection,
    },
    {
        name: tm("flow.nodes")[6],
        type: "ExternalHttpNode",
        desc: "Request and send data to external HTTP API with variables",
        icon: EpConnection,
    },
    {
        name: tm("flow.nodes")[7],
        type: "SendEmailNode",
        desc: "Sending an email an many recipients",
        icon: EpMessage,
    },
    {
        name: tm("flow.nodes")[3],
        type: "GotoNode",
        desc: tm("flow.nodesDesc")[3],
        icon: EpDArrowRight,
    },
    {
        name: tm("flow.nodes")[8],
        type: "EndNode",
        desc: "Ending node",
        icon: EpCircleClose,
    },
];
let selectedSubFlowIdx = -1;
// let offsetLeft = 0;
// let offsetTop = 0;
let graph = null;
let editedSubFlow = false;
const mainFlowId = route.params.id;
const mainFlowName = atob(route.params.name);
const isDemo = mainFlowId.indexOf("demo") > -1;

onMounted(async () => {
    // console.log("subflow onMounted");
    const canvas = document.getElementById("canvas");
    // offsetLeft = canvas.offsetLeft;
    // console.log('offsetLeft=' + offsetLeft);
    // offsetTop = canvas.offsetTop;
    // console.log('offsetTop=' + offsetTop);
    // console.log('offsetHeight=' + canvas.offsetHeight);
    graph = new Graph({
        container: canvas,
        width: "100%",
        // width: canvas.offsetWidth - 10,
        height: "100%",
        // height: canvas.offsetHeight,
        // height: 500,
        background: {
            color: "#f7f8fa",
        },
        grid: {
            visible: false,
            size: 1,
        },
        autoResize: false,
        connecting: {
            allowBlank: false,
            allowLoop: false,
            allowNode: true,
            allowMulti: true,
            connector: { name: "smooth" },
            // http://x6.antv.antgroup.com/tutorial/basic/interacting#createedge
            createEdge() {
                return this.createEdge({
                    shape: "edge",
                    attrs: {
                        line: {
                            stroke: "#a2a9b8",
                            strokeWidth: 1.5,
                            targetMarker: {
                                name: "block",
                                width: 10,
                                height: 7,
                            },
                        },
                    },
                });
            },
        },
        // https://x6.antv.vision/zh/docs/api/graph/interaction#highlighting
        // 可以通过 graph.options.highlighting.magnetAvailable.attrs.xxx = xxx 动态修改样式。
        highlighting: {
            // 当链接桩可以被链接时，在链接桩外围渲染一个 2px 宽的高亮框
            magnetAvailable: {
                name: "stroke",
                args: {
                    padding: 4,
                    attrs: {
                        "stroke-width": 2,
                        stroke: "#626aef",
                    },
                },
            },
        },
        panning: true,
    });
    graph.on("node:click", ({ e, x, y, node, view }) => {
        node.setTools([
            {
                name: "button-remove",
                args: { x: 0, y: 0 },
            },
        ]);
    });
    graph.on("node:mouseleave", ({ e, x, y, node, view }) => {
        if (node.hasTool("button-remove")) {
            node.removeTool("button-remove");
        }
    });
    graph.on("node:dblclick", ({ e, x, y, node, view }) => {
        node.setData({ currentTime: Date.now() });
        editedSubFlow = true;
    });
    graph.on("edge:click", ({ e, x, y, edge, view }) => {
        edge.setTools(["button-remove"]);
    });

    const t = await httpReq(
        "GET",
        "subflow",
        { robotId: robotId, mainFlowId: mainFlowId, data: "" },
        null,
        null,
    );
    if (isDemo) {
        const d = { status: 200, data: t };
        cacheSubFlows(d);
    } else cacheSubFlows(t);
    nextTick(() => {
        showSubFlow(0);
    });
    // console.log("onMounted2");
});

onUnmounted(() => {
    if (graph != null) graph.dispose();
});

function addHandleNode(x, y, item) {
    // console.log('addHandleNode' + x);
    const node = graph.addNode({
        shape: item.type,
        x: x,
        y: y,
        // tools: ["button-remove"],
    });
    node.setData({ nodeType: item.type });
    editedSubFlow = true;
}

function handleDragEnd(e, item) {
    const point = graph.pageToLocal(e.pageX, e.pageY);
    // addHandleNode(e.pageX - 150, e.pageY - 40, item);
    addHandleNode(point.x, point.y, item);
}

function dragoverDiv(ev) {
    ev.preventDefault();
}

function cacheSubFlows(t) {
    if (t && t.status == 200 && t.data) {
        subFlows.value = t.data;
        // showSubFlow(selectedSubFlowIdx);
    }
}

const dialogFormVisible = ref(false);
const flowName = ref("");
async function newSubFlow() {
    await saveSubFlow();
    const t = await httpReq(
        "POST",
        "subflow/new",
        { robotId: robotId, mainFlowId: mainFlowId, data: flowName.value },
        null,
        null,
    );
    if (t.status == 200) {
        const idx = subFlows.value.length;
        cacheSubFlows(t);
        nextTick(() => {
            showSubFlow(idx);
            flowName.value = "";
        });
    }
}

function removeSubFlow(index) {
    if (subFlows.value.length < 2) {
        ElMessage.error(t("flow.needOne"));
    } else {
        ElMessageBox.confirm(t("flow.delConfirm"), "Warning", {
            confirmButtonText: t("common.del"),
            cancelButtonText: t("common.cancel"),
            type: "warning",
        })
            .then(async () => {
                const r = await httpReq(
                    "DELETE",
                    "subflow",
                    {
                        robotId: robotId,
                        mainFlowId: mainFlowId,
                        data: selectedSubFlowIdx,
                    },
                    null,
                    null,
                );
                if (r.status == 200) {
                    selectedSubFlowIdx = -1;
                    subFlows.value.splice(index, 1);
                    showSubFlow(0);
                }
                ElMessage({
                    type: "success",
                    message: t("common.deleted"),
                });
            })
            .catch(() => {
                // ElMessage({
                //     type: 'info',
                //     message: 'Delete canceled',
                // })
            });
    }
}

async function showSubFlow(idx) {
    if (idx == selectedSubFlowIdx) return;
    if (editedSubFlow) {
        // console.log('editedSubFlow')
        ElMessageBox.confirm(t("flow.changeSaveTip"), "Warning", {
            confirmButtonText: t("common.save"),
            cancelButtonText: t("common.cancel"),
            type: "warning",
        })
            .then(async () => {
                await saveSubFlow();
                switchSubFlow(idx);
                editedSubFlow = false;
            })
            .catch(() => {
                switchSubFlow(idx);
                editedSubFlow = false;
            });
    } else switchSubFlow(idx);
}

function switchSubFlow(idx) {
    const o = document.getElementById(subFlowId(selectedSubFlowIdx));
    if (o) o.classList.remove("activeSubFlow");
    // console.log(idx);
    selectedSubFlowIdx = idx;
    // console.log(subFlowId(selectedSubFlowIdx));
    const n = document.getElementById(subFlowId(selectedSubFlowIdx));
    if (n) n.classList.add("activeSubFlow");
    // console.log(subFlows.value[selectedSubFlowIdx].canvas);
    // console.log(selectedSubFlowIdx);
    if (subFlows.value[selectedSubFlowIdx].canvas) {
        const canvas = JSON.parse(subFlows.value[selectedSubFlowIdx].canvas);
        const cells = canvas.cells;
        // subFlows.value[selectedSubFlowIdx].canvas = canvas;
        // console.log(subFlows.value[selectedSubFlowIdx].canvas);
        graph.fromJSON(cells);
    } else {
        graph.clearCells();
    }
}

async function saveSubFlow() {
    loading.value = true;
    saveLoading.value = true;
    const canvas = graph.toJSON();
    // console.log(canvas);
    const cells = canvas.cells;
    cells.forEach(function (item, index, arr) {
        if (item.shape != "edge") {
            item.data.nodeId = item.id;
        }
    }, nodes);
    const source = subFlows.value[selectedSubFlowIdx];
    const data = {
        valid: false,
        id: source.id,
        name: source.name,
        canvas: JSON.stringify(canvas),
        // nodes: JSON.stringify(nodes),
    };
    const r = await httpReq(
        "POST",
        "subflow",
        { robotId: robotId, mainFlowId: mainFlowId, data: selectedSubFlowIdx },
        null,
        data,
    );
    // console.log(r);
    cacheSubFlows(r);
    ElNotification({
        title: t("common.successTip"),
        message: h("b", { style: "color: teal" }, t("common.saved")),
        type: "success",
    });
    saveLoading.value = false;
    loading.value = false;
    editedSubFlow = false;
}

function subFlowId(idx) {
    return "subFlow" + idx.toString();
}

function goBack() {
    if (isDemo) router.go(-1);
    else router.push({ name: "mainflows", params: { robotId: robotId } });
}

async function release() {
    loading.value = true;
    releaseLoading.value = true;
    if (!isDemo) {
        await saveSubFlow();
    }
    const r = await httpReq(
        "GET",
        "mainflow/release",
        { robotId: robotId, mainFlowId: mainFlowId, data: "" },
        null,
        null,
    );
    // console.log(r);
    if (r.status == 200) {
        ElNotification({
            title: t("common.successTip"),
            message: h(
                "b",
                { style: "color: teal" },
                t("flow.subFlowReleased"),
            ),
            type: "success",
        });
    } else {
        ElNotification({
            title: t("common.errTip"),
            message: h("b", { style: "color: teal" }, r.err.message),
            type: "error",
        });
    }
    releaseLoading.value = false;
    loading.value = false;
}

// const formLabelWidth = '90px'
const loading = ref(false);
const saveLoading = ref(false);
const releaseLoading = ref(false);
const waitingResponse = ref(false);

const dryrunDisabled = ref(false);
const chatScrollbarRef = ref();
const dryrunChatRecords = ref();
const testingFormVisible = ref(false);
const userAsk = ref("");
const chatRecords = ref([]);
const pendingImages = ref([]);
const dryrunImageInput = ref();
let braidworkKikoSDK = null;
function onDryrunImagesSelected(e) {
    const files = Array.from(e.target.files || []);
    for (const f of files) {
        if (!f.type.startsWith("image/")) continue;
        const reader = new FileReader();
        reader.onload = () => {
            // reader.result is a data URI: "data:image/jpeg;base64,..."
            const base64 = reader.result.split(",")[1];
            pendingImages.value.push({ mimeType: f.type, data: base64 });
        };
        reader.readAsDataURL(f);
    }
    e.target.value = "";
}
function removePendingImage(idx) {
    pendingImages.value.splice(idx, 1);
}
async function dryrun() {
    if (chatRecords.value.length > 0 && !userAsk.value && pendingImages.value.length == 0)
        return;
    if (waitingResponse.value) return;
    waitingResponse.value = true;
    if (braidworkKikoSDK == null) {
        braidworkKikoSDK = new BraidworkKikoSDK({
            url: import.meta.env.VITE_REQ_BACKEND_PREFIX + "flow/answer",
            robotId: robotId,
            mainFlowId: mainFlowId,
            chatHistory: chatRecords.value,
        });
    }
    await braidworkKikoSDK.sendMessage({
        type: braidworkKikoSDK.MessageKind.PLAIN_TEXT,
        content: userAsk.value,
        attachments: pendingImages.value,
    });
    if (braidworkKikoSDK.chatHasEnded) {
        braidworkKikoSDK.addChat(
            t("flow.guideReset"),
            "terminateText",
            braidworkKikoSDK.MessageKind.PLAIN_TEXT,
            -1,
        );
        dryrunDisabled.value = true;
    }
    userAsk.value = "";
    pendingImages.value.splice(0, pendingImages.value.length);
    waitingResponse.value = false;
    dryrunInput.value.focus();
    nextTick(() => {
        // console.log(dryrunChatRecords.value.clientHeight);
        chatScrollbarRef.value.setScrollTop(
            dryrunChatRecords.value.clientHeight,
        );
    });
}
/*
let sessionId = '';
function newSessionId() {
    const d = Date.now().toString();
    return d + Math.random().toString(16);
}
function addChat(t, c, aT, idx) {
    if (idx && idx > -1) {
        if (idx >= chatRecords.value.length) {
            for (let i = chatRecords.value.length; i < idx; i++) {
                chatRecords.value.push(chatRecords.value.push({
                    id: 'chat-' + Math.random().toString(16),
                    text: '',
                    textSource: c,
                    answerType: aT,
                }));
            }
        } else {
            chatRecords.value[idx].text += t;
            return idx;
        }
    }
    chatRecords.value.push({
        id: 'chat-' + Math.random().toString(16),
        text: t.trimStart(),
        textSource: c,
        answerType: aT,
    });
    return chatRecords.value.length - 1;
}
async function dryrun2() {
    if (chatRecords.value.length > 0 && !userAsk.value)
        return;
    if (waitingResponse.value)
        return;
    // console.log('ANSWER START');
    waitingResponse.value = true;
    if (userAsk.value)
        addChat(userAsk.value, 'userText', 'TextPlan');
    if (!sessionId)
        sessionId = newSessionId();
    const req = {
        robotId: robotId,
        mainFlowId: mainFlowId,
        sessionId: sessionId,
        userInputResult: chatRecords.value.length == 0 || userAsk.value ? 'Successful' : 'Timeout',
        userInput: userAsk.value,
        importVariables: [],
        // userInputIntent: '',
    };
    userAsk.value = '';
    const res = await chatReq('POST', 'flow/answer', null, null, req);
    if (res.stream) {
        let { value, done } = await res.reader.read();
        let idx = -1;
        while (!done) {
            console.log('chunk:', value);
            // console.log('idx:', idx);
            if (value === null || value === undefined || value.trim().length == 0) {
                continue;
            }
            value.substring(1, value.length - 1).split('}{').forEach((line) => {
                if (line.trim().length > 0) {
                    console.log('line:', line);
                    // const c = value.charAt(0);
                    // let j;
                    // if (c !== '{' && c !== '[') {
                    //     j = { data: { answers: [{ content: value }] } };
                    // }
                    // else
                    //     j = JSON.parse(line);
                    const j = JSON.parse('{' + line + '}');
                    if (Object.hasOwn(j, 'contentSeq') && j.contentSeq !== null) {
                        showAnswers({ status: 200, data: { answers: [{ content: j.content }] } }, j.contentSeq);
                    } else
                        idx = showAnswers({ status: 200, data: JSON.parse(j.content) }, idx);
                }
            });
            ({ value, done } = await res.reader.read());
        }
    } else {
        showAnswers(res.data, -1);
    }
    // console.log('ANSWER DONE');
    // if (res.status == 200) {
    //     const data = res.data;
    //     const answers = data.answers;
    //     let newIdx = -1;
    //     for (let i = 0; i < answers.length; i++)
    //         newIdx = addChat(answers[i].content, 'responseText', answers[i].contentType, idx);
    //     if (data.nextAction == 'Terminate') {
    //         addChat(t('flow.guideReset'), 'terminateText', 'TextPlain', idx);
    //         dryrunDisabled.value = true;
    //     }
    //     nextTick(() => {
    //         // console.log(dryrunChatRecords.value.clientHeight);
    //         chatScrollbarRef.value.setScrollTop(dryrunChatRecords.value.clientHeight);
    //     })
    // }
    waitingResponse.value = false;
    dryrunInput.value.focus();
}
function showAnswers(r, idx) {
    console.log(r);
    if (r.status == 200) {
        console.log('data.nextAction:', r.data.nextAction);
        const data = r.data;
        const answers = data.answers;
        let newIdx = -1;
        if (answers != null) {
            for (let i = 0; i < answers.length; i++)
                newIdx = addChat(answers[i].content, 'responseText', answers[i].contentType, idx);
        }
        if (data.nextAction === 'Terminate') {
            addChat(t('flow.guideReset'), 'terminateText', 'TextPlain', -1);
            dryrunDisabled.value = true;
        }
        nextTick(() => {
            // console.log(dryrunChatRecords.value.clientHeight);
            chatScrollbarRef.value.setScrollTop(dryrunChatRecords.value.clientHeight);
        })
        return newIdx;
    } else {
        ElNotification({
            title: t('common.errTip'),
            message: h('b', { style: 'color: teal' }, r.err.message),
            type: 'error',
        });
    }
    return null;
}
*/
async function dryrunClear() {
    braidworkKikoSDK = null;
    chatRecords.value.splice(0, chatRecords.value.length);
    userAsk.value = "";
    pendingImages.value.splice(0, pendingImages.value.length);
    // sessionId = '';
    dryrunDisabled.value = false;
    waitingResponse.value = false;
    await dryrun();
}

// const isEnLanguage = navigator.language ? navigator.language.split('-')[0] == 'en' : false
// const nodesBtnWidth = isEnLanguage ? ref('100px') : ref('50px')

const dryrunInput = ref();
const popupRundryWindow = async () => {
    testingFormVisible.value = true;
    await dryrun();
};
</script>
<style scoped>
.el-container,
.el-header,
.el-main,
.el-footer {
    padding: 0;
}

.el-main {
    position: relative !important;
}

/* ---------- Header ---------- */
.el-header {
    display: flex;
    align-items: center;
    background: #fff;
    border-bottom: 1px solid #eef0f4;
    box-shadow: 0 1px 4px rgba(31, 35, 41, 0.06);
    z-index: 20;
}

.el-header .el-page-header {
    flex: 1;
    align-items: center;
}

.header-actions .el-button+.el-button {
    margin-left: 0;
}

.header-title {
    font-size: 16px;
    font-weight: 600;
    color: #1f2329;
}

/* ---------- Header action buttons ---------- */
.header-actions {
    display: flex;
    align-items: center;
    gap: 10px;
}

.demo-step {
    color: #8a919f;
    font-size: 13px;
    display: inline-flex;
    align-items: center;
    gap: 4px;
}

.icon-btn {
    width: 32px;
    height: 32px;
    padding: 0;
    color: #626aef;
    background: #eef0ff;
    border: 1px solid #cdd1fd;
    transition:
        background 0.2s,
        color 0.2s,
        box-shadow 0.2s,
        transform 0.15s;
}

.icon-btn:hover {
    color: #fff;
    background: #626aef;
    border-color: #626aef;
    box-shadow: 0 2px 8px rgba(98, 106, 239, 0.35);
    transform: translateY(-1px);
}

.icon-btn.release-btn {
    color: #00b578;
    background: #e8f8f0;
    border-color: #b3e6cd;
}

.icon-btn.release-btn:hover {
    color: #fff;
    background: #00b578;
    border-color: #00b578;
    box-shadow: 0 2px 8px rgba(0, 181, 120, 0.35);
}

.icon-btn.test-btn {
    color: #fff;
    background: linear-gradient(135deg, #626aef, #8b5cf6);
    border-color: transparent;
}

.icon-btn.test-btn:hover {
    color: #fff;
    background: linear-gradient(135deg, #5058e0, #7a4be6);
    box-shadow: 0 2px 10px rgba(98, 106, 239, 0.45);
}

.icon-btn.el-button.is-loading {
    color: #626aef;
}

/* ---------- Sidebar ---------- */
.el-aside {
    background: #fff;
    border-right: 1px solid #eef0f4;
    padding: 12px 10px;
    box-sizing: border-box;
    overflow-y: auto;
}

.aside-title {
    font-size: 12px;
    font-weight: 600;
    color: #8a919f;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    padding: 4px 6px 8px;
}

.newSubFlowBtn {
    display: flex;
    align-items: center;
    gap: 6px;
    color: #626aef;
    padding: 9px 10px;
    border-radius: 8px;
    background: linear-gradient(135deg, #eef0ff, #f7f0ff);
    border: 1px dashed #b9befc;
    cursor: pointer;
    font-size: 13px;
    font-weight: 500;
    transition:
        background 0.2s,
        box-shadow 0.2s,
        transform 0.15s;
    user-select: none;
}

.newSubFlowBtn:hover {
    background: linear-gradient(135deg, #e2e5ff, #f0e4ff);
    box-shadow: 0 2px 8px rgba(98, 106, 239, 0.2);
    transform: translateY(-1px);
}

.subFlowBtn {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 4px;
    padding: 8px 10px;
    margin-top: 6px;
    border-radius: 8px;
    cursor: pointer;
    font-size: 13px;
    color: #4e5969;
    border: 1px solid transparent;
    transition:
        background 0.2s,
        color 0.2s,
        border-color 0.2s;
    word-break: break-all;
}

.subFlowBtn:hover {
    background: #f5f6f9;
    color: #626aef;
}

.subFlowBtn.activeSubFlow {
    background: #eef0ff;
    color: #626aef;
    border-color: #cdd1fd;
    font-weight: 600;
}

.subFlowBtn .el-icon {
    opacity: 0;
    flex-shrink: 0;
    transition: opacity 0.2s;
}

.subFlowBtn:hover .el-icon {
    opacity: 0.65;
}

.subFlowBtn .el-icon:hover {
    opacity: 1;
    color: #f56c6c;
}

/* ---------- Node palette ---------- */
.nodesBox {
    display: flex;
    flex-direction: column;
    position: absolute;
    top: 20px;
    left: 20px;
    z-index: 100;
    width: 116px;
    padding: 10px 8px;
    border-radius: 12px;
    background: rgba(255, 255, 255, 0.82);
    backdrop-filter: blur(8px);
    -webkit-backdrop-filter: blur(8px);
    border: 1px solid rgba(255, 255, 255, 0.9);
    box-shadow: 0 8px 24px rgba(31, 35, 41, 0.1);
}

.palette-title {
    font-size: 11px;
    font-weight: 600;
    color: #8a919f;
    text-align: center;
    padding-bottom: 8px;
    letter-spacing: 0.5px;
}

.node-btn {
    cursor: grab;
    border: 1px solid #eef0f4;
    padding: 9px 10px;
    margin-bottom: 6px;
    font-size: 12px;
    width: 116px;
    box-sizing: border-box;
    border-radius: 8px;
    background-color: #fff;
    box-shadow: 0 1px 2px rgba(31, 35, 41, 0.05);
    transition:
        transform 0.15s,
        box-shadow 0.15s,
        border-color 0.15s;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    border-left-width: 4px;
    user-select: none;
}

.node-btn:hover {
    transform: translateY(-2px);
    box-shadow: 0 4px 12px rgba(31, 35, 41, 0.12);
}

.node-btn:active {
    cursor: grabbing;
}

.node-btn-label {
    display: flex;
    align-items: center;
    gap: 5px;
    min-width: 0;
}

.node-btn-icon {
    flex: none;
}

.DialogNode {
    border-left-color: #ff9500;
}

.KnowledgeBaseAnswerNode {
    border-left-color: #f43f5e;
}

.ConditionNode {
    border-left-color: #65a30d;
}

.CollectNode {
    border-left-color: #0ea5e9;
}

.GotoNode {
    border-left-color: #10b981;
}

.ExternalHttpNode {
    border-left-color: #0891b2;
}

.SendEmailNode {
    border-left-color: #ef4444;
}

.EndNode {
    border-left-color: #3b4a8f;
}

.LlmChatNode {
    border-left-color: #7c3aed;
}

/* ---------- Canvas ---------- */
#canvas {
    height: calc(100vh - 43px);
    border: none !important;
}

/* ---------- Dry-run chat drawer ---------- */
.chat-record {
    margin-bottom: 14px;
}

.chat-record.userText {
    text-align: right;
}

.chat-record.userText span {
    display: inline-block;
    max-width: 80%;
    padding: 8px 12px;
    border-radius: 12px 12px 2px 12px;
    background: linear-gradient(135deg, #626aef, #8b5cf6);
    color: #fff;
    text-align: left;
    white-space: pre-wrap;
    word-break: break-word;
    box-shadow: 0 2px 6px rgba(98, 106, 239, 0.25);
}

.chat-record.responseText {
    text-align: left;
}

.chat-record.responseText span {
    display: inline-block;
    max-width: 80%;
    padding: 8px 12px;
    border-radius: 12px 12px 12px 2px;
    background: #fff;
    border: 1px solid #eef0f4;
    color: #1f2329;
    white-space: pre-wrap;
    word-break: break-word;
    box-shadow: 0 1px 3px rgba(31, 35, 41, 0.06);
}

.chat-record.terminateText {
    text-align: center;
}

.chat-record.terminateText span {
    display: inline-block;
    padding: 5px 14px;
    border-radius: 999px;
    background: #f2f3f5;
    border: 1px solid #e5e6eb;
    color: #86909c;
    font-size: 12px;
    white-space: pre-wrap;
    word-break: break-word;
}

.chat-record .chat-images {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
    margin-bottom: 4px;
}
</style>
<template>
    <div>
        <!-- <div id="modal-container"></div> -->
        <el-container style="min-height: 100vh; max-height: 100vh">
            <el-header height="40px">
                <el-page-header :title="t('common.back')" @back="goBack">
                    <template #content>
                        <span class="header-title mr-3">{{
                            mainFlowName
                            }}</span>
                    </template>
                    <template #extra>
                        <div class="header-actions">
                            <el-text v-show="isDemo" class="demo-step">{{
                                $tm("flow.steps")[0]
                                }}</el-text>
                            <el-tooltip :content="$t('flow.save')" placement="bottom">
                                <el-button class="icon-btn" circle @click="saveSubFlow" :loading="saveLoading"
                                    v-show="!isDemo">
                                    <el-icon :size="17">
                                        <EpEdit />
                                    </el-icon>
                                </el-button>
                            </el-tooltip>
                            <el-tooltip :content="$t('flow.pub')" placement="bottom">
                                <el-button class="icon-btn release-btn" circle @click="release"
                                    :loading="releaseLoading">
                                    <el-icon :size="17">
                                        <EpFinished />
                                    </el-icon>
                                </el-button>
                            </el-tooltip>
                            <el-text v-show="isDemo" class="demo-step">{{
                                $tm("flow.steps")[1]
                                }}</el-text>
                            <el-tooltip :content="$t('flow.test')" placement="bottom">
                                <el-button class="icon-btn test-btn" circle @click="popupRundryWindow">
                                    <el-icon :size="17">
                                        <EpPromotion />
                                    </el-icon>
                                </el-button>
                            </el-tooltip>
                        </div>
                    </template>
                </el-page-header>
            </el-header>
            <el-container>
                <el-aside width="170px">
                    <div class="aside-title">{{
                        $t("flow.subFlowList")
                        }}</div>
                    <div class="newSubFlowBtn" @click="dialogFormVisible = true">
                        <el-icon size="16px">
                            <EpPlus />
                        </el-icon>
                        {{ $t("flow.addSubFlow") }}
                    </div>
                    <div v-for="(item, index) in subFlows" :id="subFlowId(index)" :key="item.label"
                        @click="showSubFlow(index)" class="subFlowBtn">
                        <span>{{ item.name }}</span>
                        <span @click="removeSubFlow(index)">
                            <el-icon>
                                <EpDelete />
                            </el-icon>
                        </span>
                    </div>
                </el-aside>
                <el-main v-loading="loading">
                    <div class="nodesBox">
                        <div class="palette-title">{{
                            $t("flow.nodePalette")
                            }}</div>
                        <div v-for="item in nodes" :key="item.type" class="node-btn" :class="item.type" draggable="true"
                            @dragend="handleDragEnd($event, item)">
                            <el-tooltip class="box-item" effect="dark" :content="item.desc" placement="right-start">
                                <span class="node-btn-label">
                                    <el-icon :size="13" class="node-btn-icon">
                                        <component :is="item.icon" />
                                    </el-icon>
                                    {{ item.name }}
                                </span>
                            </el-tooltip>
                        </div>
                    </div>
                    <div id="canvas" @dragover="dragoverDiv"></div>
                    <TeleportContainer />
                </el-main>
            </el-container>
        </el-container>
        <el-dialog v-model="dialogFormVisible" :title="$t('flow.addSubFlow')">
            <el-form :model="form">
                <el-form-item :label="t('flow.form.name')" label-width="110px">
                    <el-input v-model="flowName" autocomplete="off" />
                </el-form-item>
            </el-form>
            <template #footer>
                <span class="dialog-footer">
                    <el-button type="primary" @click="
                        dialogFormVisible = false;
                    newSubFlow();
                    ">
                        {{ $t("common.add") }}
                    </el-button>
                    <el-button @click="dialogFormVisible = false">{{
                        $t("common.cancel")
                        }}</el-button>
                </span>
            </template>
        </el-dialog>
        <el-drawer v-model="testingFormVisible" direction="rtl" size="50%" :append-to-body="true"
            :destroy-on-close="true">
            <template #header>
                <b>{{ $t("flow.test") }}</b>
            </template>
            <template #default>
                <el-scrollbar ref="chatScrollbarRef" height="100%" always>
                    <div ref="dryrunChatRecords">
                        <div v-for="item in chatRecords" :key="item.id" class="chat-record" :class="item.textSource">
                            <!-- <span v-html="item.text"></span> -->
                            <div v-if="item.images && item.images.length" class="chat-images">
                                <el-image v-for="(img, imgIdx) in item.images" :key="imgIdx" :src="img"
                                    :preview-src-list="item.images" :initial-index="imgIdx" fit="cover"
                                    style="width: 96px; height: 96px; border-radius: 4px" />
                            </div>
                            <el-text v-if="item.answerType == 'TextPlain'">{{
                                item.text
                                }}</el-text>
                            <el-text v-else v-html="item.text"></el-text>
                        </div>
                    </div>
                </el-scrollbar>
            </template>
            <template #footer>
                <div style="flex: auto">
                    <div v-if="pendingImages.length"
                        style="margin-bottom: 6px; display: flex; gap: 6px; flex-wrap: wrap">
                        <div v-for="(img, idx) in pendingImages" :key="idx" style="position: relative">
                            <el-image :src="`data:${img.mimeType};base64,${img.data}`" fit="cover"
                                style="width: 48px; height: 48px; border-radius: 4px" />
                            <el-button size="small" type="danger" circle style="
                                    position: absolute;
                                    top: -6px;
                                    right: -6px;
                                    transform: scale(0.7);
                                " @click="removePendingImage(idx)">×</el-button>
                        </div>
                    </div>
                    <div style="display: flex; align-items: center; gap: 8px">
                        <el-input ref="dryrunInput" :disabled="dryrunDisabled" v-model="userAsk" placeholder=""
                            style="flex: 1" @keypress="
                                (e) => {
                                    if (e.keyCode == 13) dryrun();
                                }
                            " />
                        <el-button-group>
                            <el-button :disabled="dryrunDisabled" @click="dryrunImageInput.click()">🖼</el-button>
                            <el-button type="primary" :disabled="dryrunDisabled" @click="dryrun"
                                :loading="waitingResponse">{{
                                    waitingResponse ? "Sending" : $t("flow.send")
                                }}</el-button>
                            <el-button @click="dryrunClear">{{
                                $t("flow.reset")
                                }}</el-button>
                        </el-button-group>
                    </div>
                    <input ref="dryrunImageInput" type="file" accept="image/*" multiple style="display: none"
                        @change="onDryrunImagesSelected" />
                </div>
            </template>
        </el-drawer>
    </div>
</template>
