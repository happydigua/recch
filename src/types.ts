export interface ConnectionConfig {
    id: string
    name: string
    db_type: 'mysql' | 'postgresql' | 'redis'
    host: string
    port: number
    username?: string
    password?: string
    database?: string
}
