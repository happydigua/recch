
export const isTauri = () => {
    return typeof window !== 'undefined' && '__TAURI__' in window;
};

export const invoke = async <T>(cmd: string, args?: Record<string, unknown>): Promise<T> => {
    if (!isTauri()) {
        console.warn(`[Tauri] Mock invoke for command "${cmd}"`, args);
        return Promise.reject(new Error("Tauri not available"));
    }

    // Dynamic import to avoid build errors in non-Tauri envs if package is strictly typed
    const { invoke } = await import('@tauri-apps/api/core');
    return invoke(cmd, args);
};
