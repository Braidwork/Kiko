<script setup>
import { inject, reactive, ref, onMounted } from "vue";
// import { ElMessageBox } from 'element-plus'
import {
    copyProperties,
    getDefaultBranch,
    httpReq,
} from "../../../assets/tools.js";
import { useI18n } from "vue-i18n";
import EpWarning from "~icons/ep/warning";
import EpConnection from "~icons/ep/connection";
const { t, tm, rt } = useI18n();
const nodeSetFormVisible = ref(false);
const getNode = inject("getNode");
const { robotId } = inject("robotId");
const allNodeNameSet = inject("allNodeNameSet");
const node = getNode();
const formLabelWidth = "130px";
const apis = reactive([]);
const nodeName = ref();
const apisRef = ref();
let originAsyncReqSetting = false;
const nodeData = reactive({
    nodeName: "ExternalHttpNode",
    httpApiName: "",
    httpApiId: "",
    connectTimeoutMilliseconds: 1000,
    readTimeoutMilliseconds: 1500,
    asyncReq: false,
    valid: false,
    invalidMessages: [],
    newNode: true,
    branches: [],
});
const validate = () => {
    const d = nodeData;
    const m = d.invalidMessages;
    m.splice(0, m.length);
    if (nodeData.httpApiName == "" || nodeData.httpApiId == "")
        m.push("Please choose a HTTP interface");
    if (getNode().getPortAt(0).id == "")
        m.push('Please connect "Next" to another node');
    d.valid = m.length == 0;
};
const saveForm = () => {
    // const port = getNode().getPortAt(0);
    // const branch = nodeData.branches[0];
    // branch.branchName = 'Next';
    // branch.branchId = port.id;
    addBranches();
    validate();
    // console.log(this.nodeData);
    node.removeData({ silent: true });
    node.setData(nodeData, { silent: false });
    hideForm();
};
const showOptions = (v) => {
    for (var i in apis) {
        if (apis[i].id == v) {
            nodeData.httpApiName = apis[i].name;
            break;
        }
    }
};
const hideForm = () => {
    nodeSetFormVisible.value = false;
};
node.on("change:data", ({ current }) => {
    nodeSetFormVisible.value = true;
    originAsyncReqSetting = node.getData().asyncReq;
});
const addBranches = () => {
    if (originAsyncReqSetting == nodeData.asyncReq) return;
    node.removePorts();
    nodeData.branches = [];
    if (nodeData.asyncReq) {
        node.addPort({
            group: "absolute",
            args: {
                x: nodeName.value.offsetWidth - 15,
                y: nodeName.value.offsetHeight + 50,
            },
            attrs: {
                text: {
                    text: "Next",
                    fontSize: 12,
                },
            },
        });
        nodeData.branches.push(getDefaultBranch());
        const port = node.getPortAt(0);
        const branch = nodeData.branches[0];
        branch.branchName = port.attrs.text.text;
        branch.branchId = port.id;
    } else {
        node.addPort({
            group: "absolute",
            args: {
                x: nodeName.value.offsetWidth - 15,
                y: nodeName.value.offsetHeight + 40,
            },
            attrs: {
                text: {
                    text: "Successful",
                    fontSize: 12,
                },
            },
        });
        node.addPort({
            group: "absolute",
            args: {
                x: nodeName.value.offsetWidth - 15,
                y: nodeName.value.offsetHeight + 56,
            },
            attrs: {
                text: {
                    text: "Failed",
                    fontSize: 12,
                },
            },
        });
        nodeData.branches.push(getDefaultBranch());
        nodeData.branches.push(getDefaultBranch());
        let port = node.getPortAt(0);
        let branch = nodeData.branches[0];
        branch.branchName = port.attrs.text.text;
        branch.branchId = port.id;
        port = node.getPortAt(1);
        branch = nodeData.branches[1];
        branch.branchName = port.attrs.text.text;
        branch.branchId = port.id;
    }
};
onMounted(async () => {
    // console.log('httpNode')
    const r = await httpReq(
        "GET",
        "external/http",
        { robotId: robotId },
        null,
        null,
    );
    // console.log(t);
    if (r.status == 200) {
        for (var x in r.data) {
            if (r.data.hasOwnProperty(x)) {
                // console.log(t.data[x])
                apis.push(r.data[x]);
            }
        }
    }
    copyProperties(node.getData(), nodeData);
    if (nodeData.newNode) {
        let n = null;
        do {
            n =
                n == null
                    ? Date.now().toString(16)
                    : Math.random().toString(16).substring(2);
            nodeData.nodeName = t("externalHttpNode.nodeName") + "-" + n;
        } while (allNodeNameSet.value.has(nodeData.nodeName));
    }
    allNodeNameSet.value.add(nodeData.nodeName);
    nodeData.newNode = false;
    // addBranches()
});
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
    background: linear-gradient(135deg, #06b6d4, #0891b2);
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
                    <EpConnection />
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
        <div class="nodeBody">Call Http: {{ nodeData.httpApiName }}</div>
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
                label-width="70px"
                :model="nodeData"
                style="max-width: 850px"
            >
                <el-form-item
                    :label="t('common.nodeName')"
                    :label-width="formLabelWidth"
                >
                    <el-input v-model="nodeData.nodeName" />
                </el-form-item>
                <el-form-item label="HTTP APIs" :label-width="formLabelWidth">
                    <el-select
                        ref="apisRef"
                        v-model="nodeData.httpApiId"
                        placeholder="Choose an http interface"
                        @change="(v) => showOptions(v)"
                    >
                        <el-option
                            v-for="item in apis"
                            :key="item.id"
                            :label="item.name"
                            :value="item.id"
                        />
                    </el-select>
                </el-form-item>
                <el-form-item label="Sync/Async" :label-width="formLabelWidth">
                    <el-switch
                        v-model="nodeData.asyncReq"
                        active-text="Asynchronous"
                        inactive-text="Synchronous"
                    />
                </el-form-item>
                <el-form-item
                    label="Connect timeout"
                    :label-width="formLabelWidth"
                    v-show="!nodeData.asyncReq"
                >
                    <el-input-number
                        v-model="nodeData.connectTimeoutMilliseconds"
                        :min="100"
                        :max="600000"
                    />
                    milliseconds
                </el-form-item>
                <el-form-item
                    label="Read timeout"
                    :label-width="formLabelWidth"
                >
                    <el-input-number
                        v-model="nodeData.readTimeoutMilliseconds"
                        :min="200"
                        :max="600000"
                    />
                    milliseconds
                </el-form-item>
                <!-- <el-form-item label="" :label-width="formLabelWidth">
                    <el-select ref="apisRef" v-model="nodeData.httpApiId" placeholder="Choose an http interface"
                        @change="(v) => showOptions(v)">
                        <el-option v-for="item in apis" :key="item.id" :label="item.name" :value="item.id" />
                    </el-select>
                </el-form-item> -->
                <el-form-item label="" :label-width="formLabelWidth">
                    <el-text size="large">
                        <div>
                            <strong>Please note</strong> that this is just
                            calling the interface, but the returned data will be
                            ignored.
                        </div>
                        <div>
                            If you need to obtain data, please create a variable
                            and select a certain interface as the source of the
                            data.
                        </div>
                        <div>Checkout tutorial.</div>
                    </el-text>
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
