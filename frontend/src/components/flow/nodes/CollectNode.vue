<script setup>
import { inject, reactive, ref, onMounted } from "vue";
import {
    copyProperties,
    httpReq,
    getDefaultBranch,
} from "../../../assets/tools.js";
import { useI18n } from "vue-i18n";
import EpWarning from "~icons/ep/warning";
import EpCollection from "~icons/ep/collection";
const { t, tm, rt } = useI18n();
const nodeSetFormVisible = ref(false);
const nodeData = reactive({
    nodeName: t("collectNode.nodeName"),
    collectTypeName: "",
    collectType: "",
    customizeRegex: "",
    collectSaveVarName: "",
    valid: false,
    invalidMessages: [],
    branches: [],
    newNode: true,
});
const nodeName = ref();
const getNode = inject("getNode");
const { robotId } = inject("robotId");
const allNodeNameSet = inject("allNodeNameSet");
const node = getNode();
node.on("change:data", ({ current }) => {
    nodeSetFormVisible.value = true;
});
const collectionTypes = [
    { label: tm("collectNode.cTypes")[0], value: "UserInput" },
    { label: tm("collectNode.cTypes")[1], value: "Number" },
    { label: tm("collectNode.cTypes")[2], value: "CustomizeRegex" },
];
const variables = [];
onMounted(async () => {
    // console.log('collectNode')
    const node = getNode();
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
            nodeData.nodeName = t("collectNode.nodeName") + "-" + n;
        } while (allNodeNameSet.value.has(nodeData.nodeName));
        const heightOffset = nodeName.value.offsetHeight + 50;
        const x = nodeName.value.offsetWidth - 15;
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
        nodeData.newNode = false;
        // console.log(nodeData);
    }
    allNodeNameSet.value.add(nodeData.nodeName);
    const r = await httpReq(
        "GET",
        "variable",
        { robotId: robotId },
        null,
        null,
    );
    // console.log(t);
    if (r && r.status == 200 && r.data) {
        variables.splice(0, variables.length);
        r.data.forEach(function (item, index, arr) {
            this.push({ label: item.varName, value: item.varName });
        }, variables);
    }
    validate();
});
const errors = tm("collectNode.errors");
function validate() {
    const d = nodeData;
    const m = d.invalidMessages;
    m.splice(0, m.length);
    if (!d.nodeName) m.push(errors[0]);
    if (!d.collectType) m.push(errors[1]);
    if (!d.collectSaveVarName) m.push(errors[2]);
    if (d.branches == null || d.branches.length == 0) m.push(errors[3]);
    d.valid = m.length == 0;
}
function hideForm() {
    nodeSetFormVisible.value = false;
}
function saveForm() {
    setCollectTypeName();
    const node = getNode();
    const ports = node.getPorts();
    nodeData.branches.splice(0, nodeData.branches.length);
    for (let i = 0; i < ports.length; i++) {
        const branch = getDefaultBranch();
        branch.branchName = ports[i].attrs.text.text;
        branch.branchId = ports[i].id;
        branch.branchType =
            i == 0 ? "InfoCollectedSuccessfully" : "GotoAnotherNode";
        // branch.conditionGroup[0][0].conditionType = 'UserInput';
        nodeData.branches.push(branch);
    }
    validate();
    if (nodeData.collectType == "CustomizeRegex")
        nodeData.collect_type = { CustomizeRegex: nodeData.customizeRegex };
    else nodeData.collect_type = nodeData.collectType;
    node.removeData({ silent: true });
    node.setData(nodeData, { silent: false });
    hideForm();
}
function setCollectTypeName() {
    for (let i = 0; i < collectionTypes.length; i++) {
        if (nodeData.collectType == collectionTypes[i].value) {
            nodeData.collectTypeName = collectionTypes[i].label;
        }
    }
}

const labels = tm("collectNode.labels");
const formLabelWidth = "140px";
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
    background: linear-gradient(135deg, #38bdf8, #0ea5e9);
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
    line-height: 1.8;
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
                    <EpCollection />
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
        <div class="nodeBody">
            <div>
                {{ t("collectNode.cTypeName") }}: {{ nodeData.collectTypeName }}
            </div>
            <div>
                {{ t("collectNode.varName") }}: {{ nodeData.collectSaveVarName }}
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
                <el-form-item :label="labels[0]" :label-width="formLabelWidth">
                    <el-select
                        v-model="nodeData.collectType"
                        :placeholder="labels[1]"
                    >
                        <el-option
                            v-for="item in collectionTypes"
                            :key="item.label"
                            :label="item.label"
                            :value="item.value"
                        />
                    </el-select>
                </el-form-item>
                <el-form-item
                    v-show="nodeData.collectType == 'CustomizeRegex'"
                    :label="labels[2]"
                    :label-width="formLabelWidth"
                >
                    <el-input v-model="nodeData.customizeRegex" />
                </el-form-item>
                <el-form-item :label="labels[3]" :label-width="formLabelWidth">
                    <el-select
                        v-model="nodeData.collectSaveVarName"
                        :placeholder="labels[4]"
                    >
                        <el-option
                            v-for="item in variables"
                            :key="item.label"
                            :label="item.label"
                            :value="item.value"
                        />
                    </el-select>
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
