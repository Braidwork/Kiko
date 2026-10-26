<script setup>
import { inject, onMounted, reactive, ref } from "vue";
import {
    copyProperties,
    httpReq,
    getDefaultBranch,
    providerDisplayName,
} from "../../../assets/tools.js";
import { useI18n } from "vue-i18n";
import EpWarning from "~icons/ep/warning";
import EpReading from "~icons/ep/reading";
const { t, tm, rt } = useI18n();
const getNode = inject("getNode");
const { robotId } = inject("robotId");
const allNodeNameSet = inject("allNodeNameSet");
const node = getNode();
node.on("change:data", ({ current }) => {
    nodeSetFormVisible.value = true;
});
const nodeData = reactive({
    nodeName: "Knowledge base answer node",
    recallThresholds: 85,
    noAnswerThenChoice: "GotoAnotherNode",
    noAnswerThen: null,
    alternateAnswer: "",
    retrieveAnswerSources: [],
    connectTimeout: 1000,
    readTimeout: 10000,
    branches: [],
    valid: false,
    invalidMessages: [],
    newNode: true,
});
const formFields = tm("knowledgeBaseAnswerNode.formFields");
const brief = ref("");
const modelId = ref("");
const modelName = ref("");
const settings = reactive({});
const overrideTimeoutEnabled = ref(false);
const updateBrief = () => {
    // let h = "Knowledge recall thresholds: " + nodeData.recallThresholds;
    // h += "%\nRetrieve answer from: " + nodeData.retrieveAnswerSources.join(",");
    // h += "\nWhen no knowledge is recalled then: ";
    // if (nodeData.noAnswerThenChoice == "GotoAnotherNode")
    //     h += "Goto next node.";
    // else if (nodeData.noAnswerThenChoice == "ReturnAlternateAnswerInstead") {
    //     h += 'Return "' + nodeData.alternateAnswer + '" instead.';
    // }
    let nextStep = "";
    if (nodeData.noAnswerThenChoice == "GotoAnotherNode")
        nextStep = tm("knowledgeBaseAnswerNode.fallbackSteps")[0];
    else if (nodeData.noAnswerThenChoice == "ReturnAlternateAnswerInstead") {
        nextStep = tm("knowledgeBaseAnswerNode.fallbackSteps")[1];
    }
    brief.value = t("knowledgeBaseAnswerNode.brief", {
        thresholds: nodeData.recallThresholds,
        source: nodeData.retrieveAnswerSources.join(","),
        fallbackStep: nextStep,
    });
    modelId.value = providerDisplayName(settings.chatProvider.provider.id, t);
    modelName.value = settings.chatProvider.provider.model;
};
const nodeName = ref();
const nodeBrief = ref();
onMounted(async () => {
    // const node = getNode();
    const data = node.getData();
    copyProperties(data, nodeData);
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
            if (nodeData.connectTimeout > 0 && nodeData.readTimeout > 0)
                overrideTimeoutEnabled.value = true;
            if (!nodeData.connectTimeout)
                nodeData.connectTimeout =
                    settings.chatProvider.connectTimeoutMillis;
            if (!nodeData.readTimeout)
                nodeData.readTimeout = settings.chatProvider.readTimeoutMillis;
            updateBrief();
        }
    });
    if (nodeData.newNode) {
        const heightOffset = nodeName.value.offsetHeight + 100;
        const x = nodeName.value.offsetWidth - 15;
        node.addPort({
            group: "absolute",
            args: { x: x, y: heightOffset },
            attrs: {
                text: {
                    text: "Goto next node",
                    fontSize: 12,
                },
            },
        });
        let n = null;
        do {
            n =
                n == null
                    ? Date.now().toString(16)
                    : Math.random().toString(16).substring(2);
            nodeData.nodeName = t("knowledgeBaseAnswerNode.nodeName") + "-" + n;
        } while (allNodeNameSet.value.has(nodeData.nodeName));
        nodeData.newNode = false;
    }
    allNodeNameSet.value.add(nodeData.nodeName);
    validate();
});
const validate = () => {
    nodeData.invalidMessages.splice(0, nodeData.invalidMessages.length);
    if (
        nodeData.noAnswerThenChoice == "ReturnAlternateAnswerInstead" &&
        !nodeData.alternateAnswer
    )
        nodeData.invalidMessages.push("Please enter an alternate answer.");
    if (
        nodeData.retrieveAnswerSources == null ||
        nodeData.retrieveAnswerSources.length < 1
    )
        nodeData.invalidMessages.push(
            "Please select at least one source of knowledge base answers.",
        );
    nodeData.valid = nodeData.invalidMessages.length == 0;
};
const saveForm = () => {
    const ports = node.getPorts();
    const branch = getDefaultBranch();
    branch.branchName = ports[0].attrs.text.text;
    branch.branchId = ports[0].id;
    branch.branchType = "GotoAnotherNode";
    nodeData.branches.splice(0, nodeData.branches.length, branch);
    delete nodeData.noAnswerThen;
    if (nodeData.noAnswerThenChoice == "GotoAnotherNode")
        nodeData.noAnswerThen = nodeData.noAnswerThenChoice;
    else if (nodeData.noAnswerThenChoice == "ReturnAlternateAnswerInstead")
        nodeData.noAnswerThen = {
            ReturnAlternateAnswerInstead: nodeData.alternateAnswer,
        };
    validate();
    updateBrief();
    node.removeData({ silent: true });
    node.setData(nodeData, { silent: false });
    hideForm();
};
const hideForm = () => {
    nodeSetFormVisible.value = false;
};
const formLabelWidth = "215px";
const nodeSetFormVisible = ref(false);
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
    background: linear-gradient(135deg, #fb7185, #f43f5e);
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
</style>
<template>
    <div class="nodeBox">
        <div ref="nodeName" class="nodeTitle">
            <span class="titleIcon">
                <el-icon size="12">
                    <EpReading />
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
        <div ref="nodeBrief" class="nodeBody">
            {{ brief }}
        </div>
        <!-- <teleport to="body"> -->
        <el-drawer
            v-model="nodeSetFormVisible"
            :title="nodeData.nodeName"
            direction="rtl"
            size="70%"
            :append-to-body="true"
            :destroy-on-close="true"
        >
            <el-form
                label-position="right"
                label-width="100px"
                :model="nodeData"
                style="max-width: 460px"
            >
                <el-form-item
                    :label="t('common.nodeName')"
                    :label-width="formLabelWidth"
                >
                    <el-input v-model="nodeData.nodeName" />
                </el-form-item>
                <el-form-item
                    :label="formFields[0]"
                    :label-width="formLabelWidth"
                >
                    <el-input-number
                        v-model="nodeData.recallThresholds"
                        :min="1"
                        :max="100"
                    />%
                </el-form-item>
                <el-form-item
                    :label="formFields[1]"
                    :label-width="formLabelWidth"
                >
                    <el-select
                        multiple
                        v-model="nodeData.retrieveAnswerSources"
                        style="width: 240px"
                    >
                        <el-option
                            v-for="item in ['QnA', 'Doc']"
                            :key="item"
                            :label="item"
                            :value="item"
                        />
                    </el-select>
                </el-form-item>
                <el-form-item
                    :label="formFields[2]"
                    :label-width="formLabelWidth"
                >
                    <el-radio-group v-model="nodeData.noAnswerThenChoice">
                        <el-radio value="GotoAnotherNode"
                            >Goto the next node</el-radio
                        >
                        <el-radio value="ReturnAlternateAnswerInstead"
                            >Return to the text below instead and stay at the
                            current node.</el-radio
                        >
                    </el-radio-group>
                </el-form-item>
                <el-form-item
                    label=""
                    :label-width="formLabelWidth"
                    v-show="
                        nodeData.noAnswerThenChoice ==
                        'ReturnAlternateAnswerInstead'
                    "
                >
                    <el-input
                        v-model="nodeData.alternateAnswer"
                        placeholder=""
                    />
                </el-form-item>
                <el-form-item
                    :label="formFields[3]"
                    :label-width="formLabelWidth"
                >
                    {{ modelId }} - {{ modelName }}(<router-link
                        :to="{ name: 'settings', params: { robotId: robotId } }"
                        >change</router-link
                    >)
                </el-form-item>
            </el-form>
            <div class="demo-drawer__footer">
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
