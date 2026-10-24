<script setup>
import { inject, reactive, ref, onMounted } from "vue";
import { copyProperties } from "../../../assets/tools.js";
import { useI18n } from "vue-i18n";
import EpWarning from "~icons/ep/warning";
import EpCircleClose from "~icons/ep/circle-close";
const { t, tm, rt } = useI18n();
const nodeSetFormVisible = ref(false);
const nodeData = reactive({
    nodeName: "The end",
    endingText: "",
    valid: true,
    invalidMessages: [],
    newNode: true,
});
const nodeName = ref();
const getNode = inject("getNode");
const allNodeNameSet = inject("allNodeNameSet");
const node = getNode();
// node.setData(nodeData, { silent: false });
node.on("change:data", ({ current }) => {
    nodeSetFormVisible.value = true;
});
onMounted(async () => {
    // const node = getNode();
    const data = node.getData();
    // console.log(data);
    copyProperties(data, nodeData);
    if (nodeData.newNode) {
        let n = null;
        do {
            n =
                n == null
                    ? Date.now().toString(16)
                    : Math.random().toString(16).substring(2);
            nodeData.nodeName = t("theEndNode.nodeName") + "-" + n;
        } while (allNodeNameSet.value.has(nodeData.nodeName));
        nodeData.newNode = false;
        node.setData(nodeData, { silent: true });
    }
    allNodeNameSet.value.add(nodeData.nodeName);
    // console.log(allNodeNameSet.value.size);
    validate();
});
function validate() {
    const d = nodeData;
    const m = d.invalidMessages;
    m.splice(0, m.length);
    if (d.endingText && d.endingText.length > 10000)
        m.push("The text entered cannot exceed 10,000 characters");
    d.valid = m.length == 0;
}
function hideForm() {
    nodeSetFormVisible.value = false;
}
const nodeAnswer = ref();
function saveForm() {
    // const node = getNode();
    validate();
    node.removeData({ silent: true });
    node.setData(nodeData, { silent: false });
    hideForm();
    const heightOffset =
        nodeName.value.offsetHeight + nodeAnswer.value.offsetHeight;
    console.log(heightOffset);
    node.resize(node.size().width, 20 + heightOffset, { direction: "bottom" });
}

const formLabelWidth = "90px";
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
    background: linear-gradient(135deg, #3b4a8f, #22196a);
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
                    <EpCircleClose />
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
            {{ nodeData.endingText }}
        </div>
        <!-- <teleport to="body"> -->
        <el-drawer
            v-model="nodeSetFormVisible"
            :title="nodeData.nodeName"
            direction="rtl"
            size="50%"
            :append-to-body="true"
            :destroy-on-close="true"
        >
            <el-form :label-width="formLabelWidth" :model="nodeData">
                <el-form-item
                    :label="t('common.nodeName')"
                    :label-width="formLabelWidth"
                >
                    <el-input v-model="nodeData.nodeName" />
                </el-form-item>
                <el-form-item label="Ending text" :label-width="formLabelWidth">
                    <el-input v-model="nodeData.endingText" type="textarea" />
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
