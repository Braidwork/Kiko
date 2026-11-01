// 认证状态与接口（B-34）。
//
// 会话在 HttpOnly cookie 里，前端拿不到也不该拿；这里只缓存 `/auth/me` 的结果，
// 供路由守卫与界面显示。`credentials: 'include'` 同时覆盖生产同源与开发跨源
// （开发跨源时需要在 `[server] cors_origins` 里放行前端地址）。
import { reactive } from 'vue';

const prefix = import.meta.env.VITE_REQ_BACKEND_PREFIX;

export const authState = reactive({
    user: null,
    checked: false,
});

let pending = null;

async function readJson(response) {
    try {
        return await response.json();
    } catch {
        return null;
    }
}

/** 读取当前登录用户；401 视为未登录，不抛异常。 */
export async function loadMe(force = false) {
    if (authState.checked && !force) return authState.user;
    if (!pending) {
        pending = fetch(prefix + 'auth/me', {
            method: 'GET',
            credentials: 'include',
            headers: { Accept: 'application/json' },
        })
            .then(async (response) => {
                const body = await readJson(response);
                authState.user = response.ok ? body?.data ?? null : null;
                authState.checked = true;
                return authState.user;
            })
            .catch(() => {
                authState.user = null;
                authState.checked = true;
                return null;
            })
            .finally(() => {
                pending = null;
            });
    }
    return pending;
}

export async function login(username, password) {
    const response = await fetch(prefix + 'auth/login', {
        method: 'POST',
        credentials: 'include',
        headers: {
            Accept: 'application/json',
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({ username, password }),
    });
    const body = await readJson(response);
    if (!response.ok) {
        throw new Error(body?.err?.message || `Login failed (HTTP ${response.status})`);
    }
    authState.user = body?.data ?? null;
    authState.checked = true;
    return authState.user;
}

export async function logout() {
    try {
        await fetch(prefix + 'auth/logout', {
            method: 'POST',
            credentials: 'include',
            headers: { Accept: 'application/json' },
        });
    } finally {
        authState.user = null;
        authState.checked = true;
    }
}

export async function changePassword(oldPassword, newPassword) {
    const response = await fetch(prefix + 'auth/change-password', {
        method: 'POST',
        credentials: 'include',
        headers: {
            Accept: 'application/json',
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({ oldPassword, newPassword }),
    });
    const body = await readJson(response);
    if (!response.ok) {
        throw new Error(body?.err?.message || `Password change failed (HTTP ${response.status})`);
    }
    authState.user = body?.data ?? authState.user;
    return authState.user;
}
