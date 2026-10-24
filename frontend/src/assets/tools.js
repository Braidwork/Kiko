export function cloneObj(o) {
    return JSON.parse(JSON.stringify(o));
}

export function httpReq(method, uri, query, form, body) {
    const options = {
        method: method,
        headers: {
            'Accept': 'application/json',
            'Content-Type': 'application/json'
        },
    };
    let url = import.meta.env.VITE_REQ_BACKEND_PREFIX + uri;//window.location.host
    if (query) {
        const searchParams = new URLSearchParams(query);
        url += '?' + searchParams.toString();
    }
    if (form) {
        var data = new FormData();
        for (let k of Objects.keys(form)) {
            data.append(k, form[k]);
        }
        options.body = data;
    }
    else if (body) {
        options.body = JSON.stringify(body);
        console.log(options.body);
    }
    return fetch(url, options).then(response => response.json()).catch(error => error);
}

/*
export async function chatReq(method, uri, query, form, body) {
    const options = {
        method: method,
        headers: {
            'Accept': 'application/json',
            'Content-Type': 'application/json'
        },
    };

    let url = import.meta.env.VITE_REQ_BACKEND_PREFIX + uri;
    if (query) {
        const searchParams = new URLSearchParams(query);
        url += '?' + searchParams.toString();
    }

    if (form) {
        const data = new FormData();
        for (let k of Object.keys(form)) {
            data.append(k, form[k]);
        }
        options.body = data;
        delete options.headers['Content-Type']; // FormData 不要指定 content-type
    } else if (body) {
        console.log('Request body:', JSON.stringify(body));
        options.body = JSON.stringify(body);
    }

    try {
        const response = await fetch(url, options);
        const contentType = response.headers.get('content-type') || '';
        // console.log('contentType:', contentType);

        const isStream = contentType.includes('text/event-stream') ||
            contentType.includes('application/x-ndjson') ||
            contentType.includes('text/plain');

        if (isStream) {
            const reader = response.body.getReader();
            const decoder = new TextDecoder('utf-8');
            const stream = new ReadableStream({
                async start(controller) {
                    while (true) {
                        const { done, value } = await reader.read();
                        if (done) break;
                        controller.enqueue(decoder.decode(value, { stream: true }));
                    }
                    controller.close();
                }
            });

            // ReadableStream -> text chunks
            const textStream = stream.getReader();

            return {
                stream: true,
                reader: textStream, // 可逐步 read() 得到 chunk
            };
        } else {
            // 普通 JSON 请求
            const data = await response.json();
            return { stream: false, data };
        }

    } catch (error) {
        return { stream: false, error };
    }
}
*/

export function genBranchesByNode(node) {
    if (!node || !node.ports || node.ports.length == 0)
        return [];
    const branches = [];
    node.ports.items.forEach(function (port, idx, array) {
        this.push({ branchName: port.attrs.text.text, id: port.id });
    }, branches);
    return branches;
}

// provider.id 是给程序看的（"OpenAICompatible"），不是给用户看的。
// 节点上的摘要文案用它渲染，所以要翻译一下；认不出来的 id 原样返回，
// 这样以后新增 provider 时不会显示成空白。
export function providerDisplayName(id, t) {
    switch (id) {
        case 'OpenAICompatible':
            return t('botSettings.providerOpenAiCompatible');
        case 'HuggingFace':
            return t('botSettings.providerHuggingFace');
        default:
            return id;
    }
}

/**
 * 把 `src` 上**非 null / 非 undefined** 的属性合并进 `target`（`target` 里同名的
 * null 会被**原样保留**，不会跟着 `src` 变成 null）。
 *
 * 这个语义是给"用响应 / 节点数据合并进一份带默认值的对象"用的：源里缺的字段保留
 * 目标里已有的默认值。
 *
 * 注意它的另一面：**后端明确返回的 null 永远合并不进来**。凡是有"null 本身就是
 * 一个有意义的值"（＝空 / 自动 / 未设置）的字段，都会在内存里留着上一轮的旧值。
 * 调用方要么在合并前把这类字段显式归零，要么就别用它 —— 现在的处置见
 * `components/management/Settings.vue` 的 `onMounted`（`dimensions` /
 * `dimensionsByProvider` / `indexedEmbedding` 三个字段就是先清空再合并的）。
 *
 * TODO：这个"丢 null"的行为目前是全局共用的，改掉它要先把 40 处调用点里
 * "靠它保住默认值"的场景逐个确认，暂时不动。
 */
export function copyProperties(src, target) {
    if (src == null || src == undefined)
        return;
    for (const [key, value] of Object.entries(src)) {
        if (value != null && value != undefined)
            target[key] = value;
    }
}

export function getDefaultBranch() {
    return {
        branchId: '',
        branchName: '',
        branchType: 'Condition',
        targetNodeId: '',
        conditionGroup: [
            [
                {
                    conditionType: 'UserInput',
                    refOptions: [],
                    refChoice: '',
                    compareOptions: [],
                    compareType: 'Eq',
                    targetOptions: [],
                    // targetChoice: '',
                    targetValueVariant: 'Const',
                    targetValue: '',
                    inputVariable: false,
                    caseSensitiveComparison: true,
                }
            ]
        ],
        editable: true,
    };
}

function constructBranchInvalidMessage(idx, m, gIdx, cIdx, cM) {
    let errMsg;
    if (cM)
        errMsg = 'No. ' + (idx + 1) + ' branch, condition: ' + (gIdx + 1) + '-' + (cIdx + 1) + ' ' + cM;
    else
        errMsg = 'No. ' + (idx + 1) + ' branch, ' + m;
    const ret = {
        r: false,
        m: errMsg,
    };
    return ret;
}

export function checkConditionBranches(branches) {
    for (let bIdx = 0; bIdx < branches.length; bIdx++) {
        const b = branches[bIdx];
        if (!b.branchId)
            return constructBranchInvalidMessage(bIdx, 'branchId is missing', 0, null);
        if (!b.branchName)
            return constructBranchInvalidMessage(bIdx, 'branchName is missing');
        if (b.branchType === 'GotoAnotherNode')
            continue;
        if (!b.conditionGroup || !Array.isArray(b.conditionGroup) || b.conditionGroup.length < 1)
            return constructBranchInvalidMessage(bIdx, 'conditions is missing', 0, null);
        if (!Array.isArray(b.conditionGroup[0]) || b.conditionGroup[0].length < 1)
            return constructBranchInvalidMessage(bIdx, 'conditions is missing', 0, null);
        for (let gIdx = 0; gIdx < b.conditionGroup.length; gIdx++) {
            for (let cIdx = 0; cIdx < b.conditionGroup[gIdx].length; cIdx++) {
                const c = b.conditionGroup[gIdx][cIdx];
                if (!c.conditionType)
                    return constructBranchInvalidMessage(bIdx, null, gIdx, cIdx, 'conditionType is missing');
                if (!c.compareType)
                    return constructBranchInvalidMessage(bIdx, null, gIdx, cIdx, 'compareType is missing');
                if (c.compareType !== 'Timeout') {
                    if (!c.targetValue || c.targetValue.trim() === '') {
                        return constructBranchInvalidMessage(bIdx, null, gIdx, cIdx, 'targetValue is missing');
                    }
                }
            }
        }
    }
    return {
        r: true,
    };
}

export function btoa(s) {
    return window.btoa(encodeURIComponent(s));
}

export function atob(s) {
    return decodeURIComponent(window.atob(s));
}

export function l(m) {
    if (!import.meta.env.PROD)
        console.log(m);
}

export function isOnGithub() {
    const u = window.location.href;
    return u.indexOf('braidwork.github.io') > -1;
}

export function persistRobotDetail(robotDetail) {
    window.sessionStorage.setItem('prd' + robotDetail.robotId, JSON.stringify(robotDetail));
}

export function getRobotName(robotId) {
    const s = window.sessionStorage.getItem('prd' + robotId);
    if (s) {
        const robotDetail = JSON.parse(s)
        return robotDetail.robotName;
    }
    return '';
}

export function getRobotType(robotId) {
    const s = window.sessionStorage.getItem('prd' + robotId);
    if (s) {
        const robotDetail = JSON.parse(s)
        return robotDetail.robotType;
    }
    return '';
}

// export function persistRobotType(robotId, robotType) {
//     window.localStorage.setItem(robotId + 'type', robotType);
// }

// export function getRobotType(robotId) {
//     return window.localStorage.getItem(robotId + 'type');
// }
