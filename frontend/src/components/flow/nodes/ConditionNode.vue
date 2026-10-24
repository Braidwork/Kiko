<script setup>
import { checkConditionBranches, cloneObj, copyProperties, getDefaultBranch, httpReq } from '../../../assets/tools.js'
import { inject, reactive, ref, onMounted } from 'vue';
// import { ElMessage } from 'element-plus';
import { useI18n } from 'vue-i18n'
import EpPlus from '~icons/ep/plus'
import EpMinus from '~icons/ep/minus'
import EpWarning from '~icons/ep/warning'
import EpDelete from '~icons/ep/delete'
import EpSwitch from '~icons/ep/switch'
const { t, tm, rt } = useI18n();
const lastTimeBranchIdMap = new Map();
const getNode = inject('getNode');
const { robotId } = inject('robotId');
const allNodeNameSet = inject('allNodeNameSet');
const nodeSetFormVisible = ref(false);
const branchSetFormVisible = ref(false);
const formLabelWidth = '85px';
const defaultCondition = getDefaultBranch().conditionGroup[0][0];
defaultCondition.conditionType = '';
// defaultCondition.compareType = '';
// function resetDefaultCondition(c) {
//     c.refOptions = [];
//     c.compareOptions = [];
//     c.targetOptions = [];
//     c.inputVariable = false;
//     c.showCaseSensitiveCheckBox = false;
//     c.conditionType = '';
//     c.compareType = '';
//     c.refChoice = '';
//     c.targetValueVariant = 'Const';
//     c.targetValue = '';
// }
// resetDefaultCondition(defaultCondition);
const fallbackBranch = getDefaultBranch();
fallbackBranch.branchName = t('common.else');
fallbackBranch.branchType = 'GotoAnotherNode';
fallbackBranch.editable = false;
// const defaultCondition = {
//     conditionType: '',
//     refChoice: '', compareType: '', targetChoice: '',
//     refOptions: [], compareOptions: [], targetOptions: [],
//     targetValue: '',
//     inputVariable: false,
// };
const defaultConditionGroup = [[]];
const types18 = tm('conditionNode.types')
const conditionTypes = [
    { label: types18[0], value: 'UserIntent' },
    // { label: 'Zero-Shot Classification', value: 'ZeroShotTextClassification' },
    { label: types18[1], value: 'UserInput' },
    { label: types18[2], value: 'FlowVariable' },
];
const refOptionsSet = {
    "FlowVariable": [],
};
const compares18 = tm('conditionNode.compares')
const compareOptionsSet = {
    "UserIntent": [{ label: compares18[0], value: 'Eq', inputType: 0, showCS: false }],
    "UserInput": [
        { label: compares18[0], value: 'Eq', inputType: 1, showCS: true },
        { label: compares18[2], value: 'Contains', inputType: 1, showCS: true },
        { label: compares18[3], value: 'Timeout', inputType: 0, showCS: false }
    ],
    "FlowVariable": [
        { label: 'Has value', value: 'HasValue', inputType: 0, showCS: false, belongsTo: 'StrNum' },
        { label: 'Does not have value', value: 'DoesNotHaveValue', inputType: 0, showCS: false, belongsTo: 'StrNum' },
        { label: 'Is empty string', value: 'EmptyString', inputType: 0, showCS: false, belongsTo: 'Str' },
        { label: compares18[0], value: 'Eq', inputType: 1, showCS: true, belongsTo: 'StrNum' },
        { label: compares18[1], value: 'NotEq', inputType: 1, showCS: true, belongsTo: 'StrNum' },
        { label: 'Contains', value: 'Contains', inputType: 1, showCS: true, belongsTo: 'Str' },
        { label: 'Not contains', value: 'NotContains', inputType: 1, showCS: true, belongsTo: 'Str' },
        { label: 'Greater than', value: 'NGT', inputType: 1, showCS: false, belongsTo: 'Num' },
        { label: 'Greater than or equal to', value: 'NGTE', inputType: 1, showCS: false, belongsTo: 'Num' },
        { label: 'Less than', value: 'NLT', inputType: 1, showCS: false, belongsTo: 'Num' },
        { label: 'Less than or equal to', value: 'NLTE', inputType: 1, showCS: false, belongsTo: 'Num' },
    ],
    "ZeroShotTextClassification": []
};
const targetOptionsSet = {
    "UserIntent": [],
};
const loading = false;
let editConditionIndex = -1;
const nodeData = reactive({
    nodeName: t('conditionNode.nodeName'),
    branches: [fallbackBranch],
    valid: false,
    invalidMessages: [],
    newNode: true,
});
const _branch = getDefaultBranch();
_branch.conditionGroup = [];
const branch = reactive(_branch);
// const branch = reactive({
//     branchId: '',
//     branchName: '',
//     branchType: 'Condition',
//     conditionGroup: [],
// });
const nodeName = ref();

getNode().on("change:data", ({ current }) => {
    nodeSetFormVisible.value = true;
});
defaultConditionGroup[0].push(cloneObj(defaultCondition));

onMounted(async () => {
    // console.log('conditionNode')
    let r = await httpReq('GET', 'intent', { robotId: robotId }, null, null);
    // console.log(t);
    if (r && r.status == 200 && r.data) {
        const d = targetOptionsSet.UserIntent;
        d.splice(0, d.length);
        r.data.forEach(function (item, index, arr) {
            // console.log(item.name)
            this.push({ label: item.intent_name, value: item.intent_name });
        }, d);
    }
    r = await httpReq('GET', 'variable', { robotId: robotId }, null, null);
    // console.log(t);
    if (r && r.status == 200 && r.data) {
        const d = refOptionsSet.FlowVariable;
        d.splice(0, d.length);
        r.data.forEach(function (item, index, arr) {
            this.push({ label: item.varName, value: item.varName, vtype: item.varType });
        }, d);
    }

    const node = getNode();
    const data = node.getData();
    console.log(data);
    copyProperties(data, nodeData);
    // console.log(nodeData);
    // if (data) {
    //     if (data.nodeName)
    //         nodeData.nodeName = data.nodeName;
    // if (data.branches)
    //     nodeData.branches = data.branches;
    //     console.log(nodeData.branches);
    // }
    if (nodeData.newNode) {
        let n = null;
        do {
            n = n == null ? Date.now().toString(16) : Math.random().toString(16).substring(2);
            nodeData.nodeName = t('conditionNode.nodeName') + '-' + n;
        } while (allNodeNameSet.value.has(nodeData.nodeName));
    } else {
        nodeData.branches.forEach(function (b, idx) {
            lastTimeBranchIdMap.set(b.branchId, idx)
        })
    }
    allNodeNameSet.value.add(nodeData.nodeName);
    nodeData.newNode = false;
    validate();
});
const errors18 = tm('conditionNode.errors')
const validateBranchName = (rule, value, callback) => {
    if (value == '')
        callback(new Error(errors18[0]));
    else {
        for (let i = 0; i < nodeData.branches.length; i++) {
            if (nodeData.branches[i].branchName == value) {
                callback(new Error(errors18[1]));
                return;
            }
        }
        callback();
    }
};
const branchValidators = reactive({
    branchName: [{ validator: validateBranchName, trigger: 'blur' }],
});

function validate() {
    const d = nodeData;
    const m = d.invalidMessages;
    m.splice(0, m.length);
    if (!d.nodeName)
        m.push(errors18[2]);
    if (d.branches.length == 0)
        m.push(errors18[3]);
    const ret = checkConditionBranches(d.branches);
    console.log(ret)
    if (!ret.r)
        m.push(ret.m);
    d.valid = m.length == 0;
}
function addNewBranch() {
    // console.log(branch);
    editConditionIndex = -1;
    branch.branchName = '';
    branch.conditionGroup = cloneObj(defaultConditionGroup);
    // resetDefaultCondition(branch.conditionGroup[0][0]);
    branchSetFormVisible.value = true;
}
function hideBranchForm() {
    branchSetFormVisible.value = false;
}
function saveBranch() {
    if (!branch.branchName) {
        ElMessage({
            message: errors18[0],
            type: 'warning',
        });
        return;
    }
    if (editConditionIndex == -1)
        nodeData.branches.unshift(cloneObj(branch));
    else {
        nodeData.branches[editConditionIndex].branchName = branch.branchName;
        console.log(branch.conditionGroup);
        nodeData.branches[editConditionIndex].conditionGroup = cloneObj(branch.conditionGroup);
    }
    hideBranchForm();
}
function editBranch(i) {
    // branch = cloneObj(nodeData.branches[i]);
    // branch = nodeData.branches[i];
    branch.branchName = nodeData.branches[i].branchName;
    branch.conditionGroup = cloneObj(nodeData.branches[i].conditionGroup);
    editConditionIndex = i;
    branchSetFormVisible.value = true;
}
function removeBranch(i) {
    ElMessageBox.confirm(
        'Do you delete this branch?',
        'Warning',
        {
            confirmButtonText: t('common.del'),
            cancelButtonText: t('common.cancel'),
            type: 'warning',
        }
    ).then(() => {
        nodeData.branches.splice(i, 1);
        hideBranchForm();
    }).catch(() => {
        // ElMessage({
        //     type: 'info',
        //     message: 'Delete canceled',
        // })
    })
}
function hideForm() {
    // const { nodeSetFormVisible } = .getNode().getData();
    // console.log(getNode().getData());
    nodeSetFormVisible.value = false;
}
function saveForm() {
    let heightOffset = nodeName.value.offsetHeight;
    const node = getNode();
    const len = nodeData.branches.length;
    // let hasFallbackBranch = false;
    // for (let i = 0; i < len; i++) {
    //     if (branch.branchType == 'GotoAnotherNode') {
    //         hasFallbackBranch = true;
    //     }
    // }
    // if (!hasFallbackBranch)
    //     nodeData.branches.push(fallbackBranch);
    const x = nodeName.value.offsetWidth - 15;
    // node.removePorts();
    let port, id;
    const newBranchIdMap = new Map()
    // console.log(node)
    for (let i = 0; i < len; i++) {
        heightOffset += 20;
        id = nodeData.branches[i].branchId
        // console.log(nodeData.branches[i].branchName + ' ' + heightOffset)
        if (id) {
            // console.log(nodeData.branches[i].branchName + ' has id ' + id)
            lastTimeBranchIdMap.delete(id)
            port = node.getPort(id)
            // console.log('n2=' + JSON.stringify(port))
            if (port)
                node.setPortProp(port.id, ['args', 'y'], heightOffset);
        } else {
            // console.log(nodeData.branches[i].branchName + ' dose not have id ')
            // nodeData.branches[i].branchId = Math.random().toString(36).substring(2);
            node.addPort({
                // id: nodeData.branches[i].branchId,
                group: 'absolute',
                args: { x: x, y: heightOffset },
                attrs: {
                    text: {
                        text: nodeData.branches[i].branchName,
                        fontSize: 12,
                    },
                },

            }, { silent: false });
            // console.log(node.ports)
            // console.log(node.ports.items[node.ports.items.length - 1].id)
            nodeData.branches[i].branchId = node.ports.items[node.ports.items.length - 1].id;
        }
        newBranchIdMap.set(nodeData.branches[i].branchId, i)
        // console.log(nodeData.branches[i].branchId);
        if (branch.branchType == 'GotoAnotherNode') {
            hasFallbackBranch = true;
        }
    }
    lastTimeBranchIdMap.forEach((value, key, map) => {
        // console.log('remove ' + key)
        node.removePort(key, { silent: false })
        lastTimeBranchIdMap.delete(key)
    })
    for (const [key, value] of newBranchIdMap) {
        lastTimeBranchIdMap.set(key, value);
    }
    node.resize(node.size().width, 20 + heightOffset, { direction: 'bottom', silent: false })
    validate();
    node.removeData({ silent: true });
    node.setData(nodeData, { silent: true });
    hideForm();
    // console.log(node.getData());
}
function showOptions(v, groupIndex, conditionIdx) {
    console.log(v + ' ' + groupIndex + ' ' + conditionIdx);
    const condition = branch.conditionGroup[groupIndex][conditionIdx];
    // condition.refChoice = '';
    // condition.compareType = '';
    // condition.targetValueVariant = 'Const';
    // condition.targetValue = '';
    // condition.inputVariable = false;
    const refOptions = refOptionsSet[v];
    if (refOptions)
        condition.refOptions = refOptions;
    else
        condition.refOptions = [];
    // compareOptions.splice(0, compareOptions.length);
    condition.compareOptions = compareOptionsSet[v];
    condition.compareType = '';
    // if (condition.compareOptions.length == 1)
    //     condition.compareType = condition.compareOptions[0].value;
    const targetOptions = targetOptionsSet[v];
    console.log(targetOptions);
    if (targetOptions)
        condition.targetOptions = targetOptions;
    else
        condition.targetOptions = [];
}
function percolateCompareOptions(compareType, groupIndex, conditionIdx, refOption) {
    const condition = branch.conditionGroup[groupIndex][conditionIdx];
    if (condition.compareOptions[0].belongsTo) {
        const selectedVar = refOptionsSet.FlowVariable.filter(function (curValue, index, arr) {
            return curValue.label === refOption;
        });
        // console.log(selectedVar);
        if (selectedVar.length == 1) {
            condition.compareType = '';
            condition.compareOptions = compareOptionsSet[compareType].filter(function (curVar, index, arr) {
                // console.log(curVar.belongsTo);
                // console.log(selectedVar[0].vtype);
                // console.log(curVar.belongsTo.indexOf(selectedVar[0].vtype));
                return curVar.belongsTo.indexOf(selectedVar[0].vtype) > -1;
            });
        }
    }
}
function selectCompareOption(groupIndex, conditionIndex, item) {
    // console.log(item.inputType);
    branch.conditionGroup[groupIndex][conditionIndex].inputVariable = item.inputType == 1;
    branch.conditionGroup[groupIndex][conditionIndex].showCaseSensitiveCheckBox = item.showCS;
}
function addContidion(conditionGroup) {
    const cond = cloneObj(defaultCondition);
    // resetDefaultCondition(cond);
    conditionGroup.push(cond);
}
function addConditionGroup() {
    // console.log(branch);
    branch.conditionGroup.push(...cloneObj(defaultConditionGroup));
    // console.log(branch);
}
function removeConditionGroup(groupIdx) {
    branch.conditionGroup.splice(groupIdx, 1);
}
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
    background: linear-gradient(135deg, #a3e635, #65a30d);
    color: #fff;
    font-weight: 600;
    font-size: 0.9rem;
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

.optionWidth {
    width: 130px;
}

/* ===== condition branch dialog ===== */
.cond-branch-dialog {
    max-width: calc(100vw - 48px);
}

.cond-branch-dialog .el-dialog__body {
    overflow-x: hidden;
}

.cond-hint {
    font-size: 12px;
    color: #4d7c0f;
    background: #f7fee7;
    border: 1px solid #ecfccb;
    border-radius: 8px;
    padding: 6px 12px;
    margin-bottom: 12px;
}

.cond-group-card {
    box-sizing: border-box;
    width: 100%;
    min-width: 0;
    border: 1px solid #eef1f6;
    border-radius: 10px;
    background: #fafbfe;
    padding: 12px 14px 4px;
}

.cond-row {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
    padding: 6px 0;
}

.cond-row :deep(.el-select),
.cond-row :deep(.el-input) {
    max-width: 100%;
}

.cond-and-badge {
    flex-basis: 100%;
    display: flex;
    align-items: center;
    gap: 10px;
    color: #65a30d;
    font-size: 12px;
    font-weight: 600;
    margin: 2px 0;
}

.cond-and-badge::before,
.cond-and-badge::after {
    content: "";
    flex: 1;
    height: 1px;
    background: dashed #ecfccb;
    background-image: linear-gradient(to right, #ecfccb 60%, transparent 40%);
    background-size: 8px 1px;
    background-repeat: repeat-x;
}

.cond-or-divider {
    display: flex;
    align-items: center;
    margin: 14px 0 4px;
}

.cond-or-badge {
    padding: 2px 14px;
    border-radius: 999px;
    background: linear-gradient(135deg, #a3e635, #65a30d);
    color: #fff;
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 1px;
    box-shadow: 0 2px 6px rgba(101, 163, 13, 0.35);
}

.cond-or-divider::before,
.cond-or-divider::after {
    content: "";
    flex: 1;
    height: 1px;
    background: #e4e7ef;
}

.cond-remove-group {
    color: #ef4444;
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 13px;
    margin-top: 6px;
}

.cond-remove-group:hover {
    color: #dc2626;
}

.cond-row-actions {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin-left: 4px;
}

.cond-add-group {
    margin-top: 14px;
    border-style: dashed;
}
</style>
<template>
    <div class="nodeBox">
        <div ref="nodeName" class="nodeTitle">
            <span class="titleIcon">
                <el-icon size="12">
                    <EpSwitch />
                </el-icon>
            </span>
            <span class="titleText">{{ nodeData.nodeName }}</span>
            <span v-show="nodeData.invalidMessages.length > 0">
                <el-tooltip class="box-item" effect="dark" :content="nodeData.invalidMessages.join('<br/>')"
                    placement="bottom" raw-content>
                    <el-icon color="red" size="16">
                        <EpWarning />
                    </el-icon>
                </el-tooltip>
            </span>
        </div>
        <!-- <teleport to="body"> -->
        <el-dialog v-model="branchSetFormVisible" :title="t('conditionNode.newBranch')" width="80%"
            class="cond-branch-dialog" :append-to-body="true" :destroy-on-close="true">
            <el-form :model="branch" :rules="branchValidators" label-position="top">
                <el-form-item :label="t('conditionNode.condName')" prop="branchName">
                    <el-input v-model="branch.branchName" autocomplete="off" minlength="1" maxlength="15"
                        style="max-width: 320px;" />
                </el-form-item>
                <div class="cond-hint">{{ t('conditionNode.andCond') }} / {{ t('conditionNode.orCond') }}</div>
                <template v-for="(g, groupIndex) in branch.conditionGroup" :key="groupIndex">
                    <div v-if="groupIndex > 0" class="cond-or-divider">
                        <span class="cond-or-badge">OR</span>
                    </div>
                    <div class="cond-group-card">
                        <div v-for="(c, index) in g" :key="index">
                            <div v-if="index > 0" class="cond-and-badge">AND</div>
                            <div class="cond-row">
                                <el-select v-model="c.conditionType" :placeholder="t('conditionNode.condTypePH')"
                                    @change="(v) => showOptions(v, groupIndex, index)" style="width:150px;">
                                    <el-option v-for="item in conditionTypes" :key="item.label" :label="item.label"
                                        :value="item.value" />
                                </el-select>
                                <el-select v-model="c.refChoice" :placeholder="t('conditionNode.comparedPH')"
                                    v-show="c.refOptions.length > 0" class="optionWidth"
                                    @change="(v) => percolateCompareOptions(c.conditionType, groupIndex, index, v)">
                                    <el-option v-for="item in c.refOptions" :key="item.label" :label="item.label"
                                        :value="item.value" />
                                </el-select>
                                <el-select v-model="c.compareType" :placeholder="t('conditionNode.compareTypePH')"
                                    v-show="c.compareOptions.length > 0" class="optionWidth">
                                    <el-option v-for="item in c.compareOptions" :key="item.label" :label="item.label"
                                        :value="item.value" @click.native="selectCompareOption(groupIndex, index, item)" />
                                </el-select>
                                <el-select v-model="c.targetValue" :placeholder="t('conditionNode.targetPH')"
                                    v-show="c.targetOptions.length > 0" class="optionWidth">
                                    <el-option v-for="item in c.targetOptions" :key="item.label" :label="item.label"
                                        :value="item.value" />
                                </el-select>
                                <el-select v-model="c.targetValueVariant" v-show="c.inputVariable" class="optionWidth">
                                    <el-option label="const value" value="Const" />
                                    <el-option label="variable value" value="Variable" />
                                </el-select>
                                <el-select v-model="c.targetValue" placeholder="Please choose a variable"
                                    v-show="c.inputVariable && c.targetValueVariant == 'Variable'" class="optionWidth">
                                    <el-option v-for="item in refOptionsSet['FlowVariable']" :key="item.label"
                                        :label="item.label" :value="item.value" />
                                </el-select>
                                <el-input v-model="c.targetValue" class="optionWidth"
                                    v-show="c.inputVariable && c.targetValueVariant == 'Const'" />
                                <el-checkbox v-show="c.showCaseSensitiveCheckBox" v-model="c.caseSensitiveComparison"
                                    label="CaseSensitive" />
                                <span class="cond-row-actions">
                                    <el-tooltip :content="t('conditionNode.andCond')" placement="top">
                                        <el-button type="primary" plain circle @click="addContidion(g)">
                                            <el-icon>
                                                <EpPlus />
                                            </el-icon>
                                        </el-button>
                                    </el-tooltip>
                                    <el-tooltip v-if="g.length > 1" :content="t('common.del')" placement="top">
                                        <el-button type="danger" plain circle @click="g.splice(index, 1)">
                                            <el-icon>
                                                <EpMinus />
                                            </el-icon>
                                        </el-button>
                                    </el-tooltip>
                                </span>
                            </div>
                        </div>
                        <span v-if="branch.conditionGroup.length > 1" class="cond-remove-group"
                            @click="removeConditionGroup(groupIndex)">
                            <el-icon>
                                <EpDelete />
                            </el-icon>
                            {{ t('common.del') }}
                        </span>
                    </div>
                </template>
                <el-button type="primary" plain class="cond-add-group" @click="addConditionGroup()">
                    <el-icon>
                        <EpPlus />
                    </el-icon>
                    {{ t('conditionNode.orCond') }}
                </el-button>
            </el-form>
            <template #footer>
                <span class="dialog-footer">
                    <el-button type="primary" @click="saveBranch()">
                        {{ t('common.save') }}
                    </el-button>
                    <el-button v-on:click="hideBranchForm()">{{ t('common.cancel') }}</el-button>
                    <el-button type="danger" :disabled="editConditionIndex == -1"
                        v-on:click="removeBranch(editConditionIndex)">{{ t('common.del') }}</el-button>
                </span>
            </template>
        </el-dialog>
        <el-drawer v-model="nodeSetFormVisible" :title="nodeData.nodeName" direction="rtl" size="70%"
            :append-to-body="true" :destroy-on-close="true">
            <el-form :model="nodeData">
                <el-form-item :label="t('common.nodeName')" :label-width="formLabelWidth">
                    <el-input v-model="nodeData.nodeName" autocomplete="off" />
                </el-form-item>
                <el-form-item :label="t('conditionNode.newCond')" :label-width="formLabelWidth">
                    <el-button type="primary" @click="addNewBranch()">
                        <el-icon>
                            <EpPlus />
                        </el-icon>
                    </el-button>
                    <el-button v-for="(btn, index) in nodeData.branches" :key="index" type="primary"
                        @click="editBranch(index)" :disabled="!btn.editable">{{ btn.branchName }}</el-button>
                </el-form-item>
            </el-form>
            <div>
                <el-button type="primary" :loading="loading" @click="saveForm()">{{ t('common.save') }}</el-button>
                <el-button @click="hideForm()">{{ t('common.cancel') }}</el-button>
            </div>
        </el-drawer>
        <!-- </teleport> -->
    </div>
</template>