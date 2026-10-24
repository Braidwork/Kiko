<script setup>
import { inject, reactive, ref, onMounted, nextTick } from "vue";
import {
    copyProperties,
    httpReq,
    getDefaultBranch,
    providerDisplayName,
} from "../../../assets/tools.js";
import { useI18n } from "vue-i18n";
import EpWarning from "~icons/ep/warning";
import EpMagicStick from "~icons/ep/magic-stick";
const { t, tm, rt } = useI18n();
const nodeData = reactive({
    nodeName: "Llm chat node",
    brief: "",
    prompt: "",
    promptText: "",
    nodeExitType: "exitByIntent",
    contextLength: 5,
    exitCondition: { "": "" },
    exitIntent: "",
    exitSpecialInputs: "",
    exitLlmResultContains: "",
    maxChatTimes: 1,
    responseStreaming: true,
    // 本地 Qwen3 的思考模式。默认关闭：小模型上思考过程会先占掉几十秒才出
    // 第一句话，而多数对话场景要的是直接回答。对非 Qwen3 模型（llama/gemma/
    // phi3，以及所有在线模型）这个开关不产生任何影响。
    enableThinking: false,
    connectTimeout: 1000,
    readTimeout: 10000,
    whenTimeoutThen: "GotoAnotherNode",
    valid: false,
    invalidMessages: [],
    branches: [],
    newNode: true,
});
const settings = reactive({});
const getNode = inject("getNode");
const { robotId } = inject("robotId");
// const { ollamaModels } = inject('ollamaModels');
const allNodeNameSet = inject("allNodeNameSet");
const nodeSetFormVisible = ref(false);
const intents = reactive([]);
const modelId = ref("");
const modelName = ref("");
const nodeName = ref();
const overrideTimeoutEnabled = ref(false);
const whenTimeoutThen = ref("GotoAnotherNode");
const responseAlternateText = ref("");
const formLabelWidth = "140px";
const formFields = tm("llmChatNode.formFields");

getNode().on("change:data", ({ current }) => {
    // console.log('toggled');
    // const { name, text, nodeSetFormVisible } = current;
    nodeSetFormVisible.value = true;
});

onMounted(async () => {
    // console.log('llmChatNode')
    const node = getNode();
    const data = node.getData();
    // console.log('data', data);
    copyProperties(data, nodeData);
    // if (data) {
    //     if (data.nodeName)
    //         nodeData.nodeName = data.nodeName;
    //     nodeData.goToType = data.goToType;
    //     selectGotoType(nodeData.goToType);
    //     nodeData.goToSubFlowId = data.goToSubFlowId;
    //     nodeData.goToSubFlowName = data.goToSubFlowName;
    //     nodeData.brief = data.brief;
    //     nodeData.externalLink = data.externalLink;
    //     if (data.newNode)
    //         nodeData.newNode = data.newNode;
    // }
    if (nodeData.newNode) {
        let n = null;
        do {
            n =
                n == null
                    ? Date.now().toString(16)
                    : Math.random().toString(16).substring(2);
            nodeData.nodeName = t("llmChatNode.nodeName") + "-" + n;
        } while (allNodeNameSet.value.has(nodeData.nodeName));
        // node.removePorts();
        node.addPort({
            group: "absolute",
            args: { x: nodeName.value.offsetWidth - 15, y: 104 },
            attrs: {
                text: {
                    text: "Next",
                    fontSize: 12,
                },
            },
        });
    }
    allNodeNameSet.value.add(nodeData.nodeName);
    nodeData.newNode = false;
    validate();
    httpReq(
        "GET",
        "management/settings",
        { robotId: robotId },
        null,
        null,
    ).then((res) => {
        // const r = res.json();
        if (res.data) {
            copyProperties(res.data, settings);
            updateBrief();
            if (nodeData.connectTimeout > 0 && nodeData.readTimeout > 0)
                overrideTimeoutEnabled.value = true;
            if (!nodeData.connectTimeout)
                nodeData.connectTimeout =
                    settings.chatProvider.connectTimeoutMillis;
            if (!nodeData.readTimeout)
                nodeData.readTimeout = settings.chatProvider.readTimeoutMillis;
        }
    });
    if (typeof nodeData.whenTimeoutThen == "string")
        whenTimeoutThen.value = nodeData.whenTimeoutThen;
    else {
        whenTimeoutThen.value = "ResponseAlternateText";
        responseAlternateText.value =
            nodeData.whenTimeoutThen.ResponseAlternateText;
    }
    const r = await httpReq("GET", "intent", { robotId: robotId }, null, null);
    // console.log(r);
    if (r.status == 200) {
        intents.splice(0, intents.length, ...r.data);
    }
});

const updateBrief = () => {
    modelId.value = providerDisplayName(settings.chatProvider.provider.id, t);
    modelName.value = settings.chatProvider.provider.model;
    // let h =
    //     "Chat model: " +
    //     settings.chatProvider.provider.model +
    //     " of " +
    //     settings.chatProvider.provider.id;
    // h += "<br/>History context length: " + nodeData.contextLength;
    // h += "<br/>Exit this node by: " + nodeData.nodeExitType.substring(6);
    let h = t("llmChatNode.brief", {
        model: settings.chatProvider.provider.model,
        id: providerDisplayName(settings.chatProvider.provider.id, t),
        contextLength: nodeData.contextLength,
        nodeExitType: nodeData.nodeExitType.substring(6),
    });
    // if (nodeData.nodeExitType == 'exitByIntent') {
    //     const intent = intents.find(i => i.intent_id == nodeData.exitIntent);
    //     h += '<br/>Intent: ' + (intent ? intent.intent_name : '');
    // } else if (nodeData.nodeExitType == 'exitBySpecialInputs') {
    //     h += '<br/>Special inputs: ' + nodeData.exitSpecialInputs;
    // } else if (nodeData.nodeExitType == 'exitByLlmResultContains') {
    //     h += '<br/>LLM result contains: ' + nodeData.exitLlmResultContains;
    // } else if (nodeData.nodeExitType == 'exitByMaxChatTimes') {
    //     h += '<br/>Max chat times: ' + nodeData.maxChatTimes;
    // }
    nodeData.brief = h;
};

const validate = () => {
    const d = nodeData;
    const m = d.invalidMessages;
    m.splice(0, m.length);
    if (!d.prompt) m.push('Please fill out "prompt" field.');
    if (d.nodeExitType == "exitByIntent" && !d.exitIntent)
        m.push("Please select an intent to use for exiting.");
    if (d.nodeExitType == "exitBySpecialInputs" && !d.exitSpecialInputs)
        m.push("Please type some special inputs for exiting.");
    if (d.nodeExitType == "exitByLlmResultContains" && !d.exitLlmResultContains)
        m.push("Please enter the string contained in the LLM result.");
    if (
        whenTimeoutThen.value == "ResponseAlternateText" &&
        !responseAlternateText.value
    )
        m.push("Please enter alternate text.");
    d.valid = m.length == 0;
};

const saveForm = () => {
    const node = getNode();
    const ports = node.getPorts();
    const branch = getDefaultBranch();
    branch.branchName = ports[0].attrs.text.text;
    branch.branchId = ports[0].id;
    branch.branchType = "GotoAnotherNode";
    nodeData.branches.splice(0, nodeData.branches.length, branch);
    validate();
    delete nodeData.exitCondition;
    nodeData.exitCondition = {};
    const nodeExitType = nodeData.nodeExitType.replace(/exitBy/, "");
    if (nodeExitType == "Intent")
        nodeData.exitCondition[nodeExitType] = nodeData.exitIntent;
    else if (nodeExitType == "SpecialInputs")
        nodeData.exitCondition[nodeExitType] = nodeData.exitSpecialInputs;
    else if (nodeExitType == "LlmResultContains")
        nodeData.exitCondition[nodeExitType] = nodeData.exitLlmResultContains;
    else if (nodeExitType == "MaxChatTimes")
        nodeData.exitCondition[nodeExitType] = nodeData.maxChatTimes;
    nodeData.prompt = JSON.stringify([
        { role: "user", content: nodeData.promptText },
    ]);
    if (!overrideTimeoutEnabled.value) {
        nodeData.connectTimeout = null;
        nodeData.readTimeout = null;
    }
    delete nodeData.whenTimeoutThen;
    if (whenTimeoutThen.value == "ResponseAlternateText") {
        nodeData.whenTimeoutThen = {
            ResponseAlternateText: responseAlternateText,
        };
    } else {
        nodeData.whenTimeoutThen = whenTimeoutThen;
    }
    console.log("nodeData", nodeData);
    node.removeData({ silent: true });
    node.setData(nodeData, { silent: false });
    updateBrief();
    hideForm();
};

const hideForm = () => {
    nodeSetFormVisible.value = false;
};
</script>
<style scoped>
.nodeBox {
    border: 1px solid #e2e8f0;
    border-radius: 10px;
    height: 100%;
    width: 100%;
    background-color: #fff;
    font-size: 12px;
    overflow: hidden;
    box-shadow: 0 2px 8px rgba(31, 45, 61, 0.08);
}

.nodeTitle {
    display: flex;
    align-items: center;
    gap: 6px;
    background: linear-gradient(135deg, #9d5ba0, #7c3aed);
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

.optionWidth {
    width: 110px;
}
</style>
<template>
    <div class="nodeBox">
        <div ref="nodeName" class="nodeTitle">
            <span class="titleIcon">
                <el-icon size="12">
                    <EpMagicStick />
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
        <!-- <div v-html="nodeData.brief"></div> -->
        <div ref="nodeBrief" class="nodeBody">
            {{ nodeData.brief }}
        </div>
        <!-- <teleport to="body"> -->
        <el-drawer
            v-if="nodeSetFormVisible"
            v-model="nodeSetFormVisible"
            :title="nodeData.nodeName"
            direction="rtl"
            size="70%"
            :append-to-body="true"
            :destroy-on-close="true"
        >
            <el-form
                label-position="right"
                label-width="135px"
                :model="nodeData"
                style="max-width: 800px"
            >
                <el-form-item
                    :label="t('common.nodeName')"
                    :label-width="formLabelWidth"
                    prop="nodeName"
                    :rules="[
                        { required: true, message: 'nodeName is required' },
                    ]"
                >
                    <el-input v-model="nodeData.nodeName" />
                </el-form-item>
                <el-form-item
                    :label="formFields[0]"
                    :label-width="formLabelWidth"
                >
                    {{ modelId }} - {{ modelName }} (<router-link
                        :to="{ name: 'settings', params: { robotId: robotId } }"
                        >change</router-link
                    >)
                </el-form-item>
                <!-- <el-form-item label="Prompt" :label-width="formLabelWidth" prop="promptText" :rules="[
                    { required: true, message: 'Prompt is required' },
                ]">
                    <el-input v-model="nodeData.promptText" :rows="6" type="textarea" placeholder="" />
                </el-form-item> -->
                <el-form-item
                    :label="formFields[1]"
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
                    :label="formFields[2]"
                    :label-width="formLabelWidth"
                >
                    <el-radio-group v-model="nodeData.nodeExitType">
                        <el-radio value="exitByIntent">Intent</el-radio>
                        <el-radio value="exitBySpecialInputs"
                            >Special inputs</el-radio
                        >
                        <el-radio value="exitByLlmResultContains"
                            >LLM result contains</el-radio
                        >
                        <el-radio value="exitByMaxChatTimes"
                            >Max chat times</el-radio
                        >
                    </el-radio-group>
                </el-form-item>
                <el-form-item label="" :label-width="formLabelWidth">
                    <el-select
                        v-model="nodeData.exitIntent"
                        v-show="nodeData.nodeExitType == 'exitByIntent'"
                    >
                        <el-option
                            v-for="item in intents"
                            :key="item.id"
                            :label="item.intent_name"
                            :value="item.intent_id"
                        />
                    </el-select>
                    <el-input
                        v-model="nodeData.exitSpecialInputs"
                        v-show="nodeData.nodeExitType == 'exitBySpecialInputs'"
                    />
                    <el-input
                        v-model="nodeData.exitLlmResultContains"
                        v-show="
                            nodeData.nodeExitType == 'exitByLlmResultContains'
                        "
                    />
                    <el-input-number
                        v-model="nodeData.maxChatTimes"
                        :min="1"
                        :max="200"
                        :step="1"
                        v-show="nodeData.nodeExitType == 'exitByMaxChatTimes'"
                    />
                </el-form-item>
                <el-form-item
                    :label="formFields[3]"
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
                    :label="formFields[4]"
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
                    :label="formFields[5]"
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
                    :label="formFields[6]"
                    :label-width="formLabelWidth"
                >
                    <el-radio-group v-model="whenTimeoutThen">
                        <el-radio value="GotoAnotherNode"
                            >Goto next node</el-radio
                        >
                        <el-radio value="ResponseAlternateText"
                            >Response alternate text</el-radio
                        >
                        <el-radio value="DoNothing">Do nothing</el-radio>
                        <el-tooltip effect="light" placement="right">
                            <template #content>
                                Stay at this node until meet one of exiting
                                condition.
                            </template>
                            <el-button size="small" circle>?</el-button>
                        </el-tooltip>
                    </el-radio-group>
                </el-form-item>
                <el-form-item
                    label="Alternate text"
                    :label-width="formLabelWidth"
                    v-show="whenTimeoutThen == 'ResponseAlternateText'"
                >
                    <el-input v-model="responseAlternateText" />
                </el-form-item>
                <el-form-item
                    :label="formFields[7]"
                    :label-width="formLabelWidth"
                >
                    <el-checkbox
                        v-model="nodeData.responseStreaming"
                        label="Response streaming"
                    />
                </el-form-item>
                <el-form-item
                    :label="formFields[8]"
                    :label-width="formLabelWidth"
                >
                    <el-checkbox
                        v-model="nodeData.enableThinking"
                        :label="formFields[8]"
                    />
                    <el-tooltip effect="light" placement="right">
                        <template #content>
                            Let the model reason step by step before answering.
                            Only affects local Qwen3 models; it makes the first
                            word take much longer to arrive. Other models ignore
                            this setting.
                        </template>
                        <el-button size="small" circle>?</el-button>
                    </el-tooltip>
                </el-form-item>
            </el-form>
            <div>
                <el-button type="primary" @click="saveForm()">{{
                    t("common.save")
                }}</el-button>
                <el-button @click="hideForm()">{{
                    t("common.cancel")
                }}</el-button>
            </div>
        </el-drawer>
        <!-- </teleport> -->
    </div>
</template>
