<script setup>
import { inject, onMounted, reactive, ref } from "vue";
import {
    copyProperties,
    getDefaultBranch,
    httpReq,
} from "../../../assets/tools.js";
import EpWarning from "~icons/ep/warning";
import EpMessage from "~icons/ep/message";
import { useI18n } from "vue-i18n";
const { t, tm } = useI18n();

const nodeData = reactive({
    nodeName: "Send email node",
    from: "",
    to: "",
    toRecipients: [],
    cc: "",
    ccRecipients: [],
    bcc: "",
    bccRecipients: [],
    subject: "",
    content: "",
    contentType: "TextHtml",
    asyncSend: true,
    valid: false,
    invalidMessages: [],
    branches: [],
    newNode: true,
});
let lastTimeAsyncSendChoice = true;
const nodeName = ref();
const smtpHost = ref("");
const emailVerificationRegex = ref("");
const nodeSetFormVisible = ref(false);
const getNode = inject("getNode");
const { robotId } = inject("robotId");
const allNodeNameSet = inject("allNodeNameSet");
const node = getNode();
const variables = [];
node.on("change:data", ({ current }) => {
    nodeSetFormVisible.value = true;
});
onMounted(async () => {
    // console.log('emailNode')
    const data = node.getData();
    // console.log(data);
    copyProperties(data, nodeData);
    // if (data) {
    //     if (data.nodeName)
    //         nodeData.nodeName = data.nodeName;
    //     nodeData.collectType = data.collectType;
    //     nodeData.collectSaveVarName = data.collectSaveVarName;
    //     if (data.newNode)
    //         nodeData.newNode = data.newNode;
    // }
    // console.log(nodeData.newNode);
    if (nodeData.newNode) {
        let n = null;
        do {
            n =
                n == null
                    ? Date.now().toString(16)
                    : Math.random().toString(16).substring(2);
            nodeData.nodeName = t("sendEmailNode.nodeName") + "-" + n;
        } while (allNodeNameSet.value.has(nodeData.nodeName));
        resetPorts();
        nodeData.newNode = false;
        // console.log(nodeData);
    } else {
        lastTimeAsyncSendChoice = nodeData.asyncSend;
    }
    allNodeNameSet.value.add(nodeData.nodeName);
    let r = await httpReq("GET", "variable", null, null, null);
    // console.log(t);
    if (r && r.status == 200 && r.data) {
        variables.splice(0, variables.length);
        r.data.forEach(function (item, index, arr) {
            this.push({ label: item.varName, value: item.varName });
        }, variables);
    }
    r = await httpReq(
        "GET",
        "management/settings",
        { robotId: robotId },
        null,
        null,
    );
    // console.log(t);
    if (r && r.status == 200 && r.data) {
        smtpHost.value = r.data.smtpHost;
        // console.log(t.data.emailVerificationRegex)
        emailVerificationRegex.value = r.data.emailVerificationRegex;
    }
    validate();
});
const resetPorts = () => {
    node.removePorts();
    const heightOffset = nodeName.value.offsetHeight + 50;
    const x = nodeName.value.offsetWidth - 15;
    if (nodeData.asyncSend) {
        node.addPort({
            group: "absolute",
            args: { x: x, y: heightOffset },
            attrs: {
                text: {
                    text: tm("dialogNode.nextSteps")[1],
                    fontSize: 12,
                },
            },
        });
    } else {
        node.addPort({
            group: "absolute",
            args: { x: x, y: heightOffset },
            attrs: {
                text: {
                    text: tm("collectNode.branches")[0],
                    fontSize: 12,
                },
            },
        });
        node.addPort({
            group: "absolute",
            args: { x: x, y: heightOffset + 20 },
            attrs: {
                text: {
                    text: tm("collectNode.branches")[1],
                    fontSize: 12,
                },
            },
        });
        node.resize(node.size().width, 40 + heightOffset, {
            direction: "bottom",
        });
    }
};
const validate = () => {
    const d = nodeData;
    const m = d.invalidMessages;
    m.splice(0, m.length);
    if (!smtpHost.value) m.push("SMTP host is not configured");
    if (!d.nodeName) m.push("Need to fill in the node name");
    // console.log(emailVerificationRegex)
    const re = new RegExp(emailVerificationRegex.value);
    if (!d.to) m.push("Need to fill in the email recipient");
    else {
        d.to.split(";").forEach(function (item) {
            if (!item.match(re)) {
                m.push(item + " is not a valid email format");
            } else d.toRecipients.push(item);
        });
    }
    if (d.cc) {
        d.cc.split(";").forEach(function (item) {
            if (!item.match(re)) {
                m.push(item + " is not a valid email format");
            } else d.ccRecipients.push(item);
        });
    }
    if (d.bcc) {
        d.bcc.split(";").forEach(function (item) {
            if (!item.match(re)) {
                m.push(item + " is not a valid email format");
            } else d.bccRecipients.push(item);
        });
    }
    if (!d.subject) m.push("Need to fill in the email subject");
    if (!d.content) m.push("Need to fill in the email content");
    if (d.branches == null || d.branches.length == 0)
        m.push("Wrong node branch information");
    // else {
    //     d.branches.forEach(function (item) {
    //         if (!item.target_node_id)
    //             m.push(item.branchName + ' is not connected to other nodes');
    //     })
    // }
    d.valid = m.length == 0;
};
const saveForm = () => {
    console.log(lastTimeAsyncSendChoice + "|" + nodeData.asyncSend);
    if (lastTimeAsyncSendChoice != nodeData.asyncSend) {
        lastTimeAsyncSendChoice = nodeData.asyncSend;
        resetPorts();
    }
    const node = getNode();
    const ports = node.getPorts();
    nodeData.branches.splice(0, nodeData.branches.length);
    for (let i = 0; i < ports.length; i++) {
        const branch = getDefaultBranch();
        branch.branchName = ports[i].attrs.text.text;
        branch.branchId = ports[i].id;
        branch.branchType =
            i == 0 && ports.length > 1
                ? "EmailSentSuccessfully"
                : "GotoAnotherNode";
        nodeData.branches.push(branch);
    }
    validate();
    node.removeData({ silent: true });
    node.setData(nodeData, { silent: false });
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
    background: linear-gradient(135deg, #ff6555, #ef4444);
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

.nodeBody {
    padding: 9px 10px;
    display: flex;
    flex-direction: column;
    gap: 5px;
    font-size: 12px;
    color: #4e5969;
    overflow: hidden;
}

.briefRow {
    display: flex;
    align-items: baseline;
    gap: 6px;
    min-width: 0;
}

.briefLabel {
    flex: none;
    color: #86909c;
}

.briefValue {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
}

.briefEmpty {
    color: #c9cdd4;
    font-style: italic;
}

.titleText {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
    min-width: 0;
}

.formHint {
    color: #86909c;
    font-size: 12px;
    line-height: 1.4;
    display: block;
    margin-top: 4px;
}

.sectionDivider {
    margin: 4px 0 18px;
}

.sectionDivider :deep(.el-divider__text) {
    padding: 0 8px;
    background-color: transparent;
}

.sectionTitle {
    color: #86909c;
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.05em;
    text-transform: uppercase;
}
</style>
<template>
    <div class="nodeBox">
        <div ref="nodeName" class="nodeTitle">
            <span class="titleIcon">
                <el-icon size="12">
                    <EpMessage />
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
                    <el-icon color="#fde047" size="16">
                        <EpWarning />
                    </el-icon>
                </el-tooltip>
            </span>
        </div>
        <div class="nodeBody">
            <div class="briefRow">
                <span class="briefLabel">To:</span>
                <span class="briefValue" :class="{ briefEmpty: !nodeData.to }">{{
                    nodeData.to || "not set"
                }}</span>
            </div>
            <div class="briefRow">
                <span class="briefLabel">Subject:</span>
                <span
                    class="briefValue"
                    :class="{ briefEmpty: !nodeData.subject }"
                    >{{ nodeData.subject || "not set" }}</span
                >
            </div>
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
                label-position="top"
                :model="nodeData"
                style="max-width: 560px"
            >
                <el-form-item :label="t('common.nodeName')" prop="nodeName">
                    <el-input v-model="nodeData.nodeName" />
                </el-form-item>
                <el-divider class="sectionDivider">
                    <span class="sectionTitle">Recipients</span>
                </el-divider>
                <el-form-item
                    label="From"
                    prop="from"
                    :rules="[
                        {
                            required: true,
                            message: 'Please input email address',
                            trigger: 'blur',
                        },
                        {
                            type: 'email',
                            message: 'Please input correct email address',
                            trigger: ['blur', 'change'],
                        },
                    ]"
                >
                    <el-input
                        v-model="nodeData.from"
                        placeholder="sender@example.com"
                    />
                </el-form-item>
                <el-form-item
                    label="To"
                    prop="to"
                    :rules="[
                        {
                            required: true,
                            message: 'Please input email address',
                            trigger: 'blur',
                        },
                        {
                            type: 'email',
                            message: 'Please input correct email address',
                            trigger: ['blur', 'change'],
                        },
                    ]"
                >
                    <el-input
                        v-model="nodeData.to"
                        placeholder="recipient@example.com;another@example.com"
                    />
                    <span class="formHint"
                        >Separate multiple recipients with semicolons</span
                    >
                </el-form-item>
                <el-form-item label="Cc">
                    <el-input
                        v-model="nodeData.cc"
                        placeholder="cc@example.com"
                    />
                </el-form-item>
                <el-form-item label="Bcc">
                    <el-input
                        v-model="nodeData.bcc"
                        placeholder="bcc@example.com"
                    />
                </el-form-item>
                <el-divider class="sectionDivider">
                    <span class="sectionTitle">Message</span>
                </el-divider>
                <el-form-item
                    label="Subject"
                    prop="subject"
                    :rules="[
                        { required: true, message: 'Subject is required' },
                    ]"
                >
                    <el-input
                        v-model="nodeData.subject"
                        placeholder="Email subject"
                    />
                </el-form-item>
                <el-form-item
                    label="Content"
                    prop="content"
                    :rules="[
                        { required: true, message: 'Content is required' },
                    ]"
                >
                    <el-input
                        v-model="nodeData.content"
                        :rows="8"
                        type="textarea"
                        placeholder="Email body..."
                    />
                </el-form-item>
                <el-form-item label="Content type">
                    <el-radio-group v-model="nodeData.contentType">
                        <el-radio-button value="TextHtml"
                            >text/html</el-radio-button
                        >
                        <el-radio-button value="TextPlain"
                            >text/plain</el-radio-button
                        >
                    </el-radio-group>
                </el-form-item>
                <el-form-item>
                    <el-switch
                        v-model="nodeData.asyncSend"
                        active-text="Send asynchronously"
                    />
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
