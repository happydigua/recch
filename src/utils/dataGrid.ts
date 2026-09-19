/** SQL builders for identifiers and JSON values returned by the database grid. */
export type SqlDialect = 'mysql' | 'postgresql'
export interface GridColumn { name: string; is_pk?: boolean }
export type GridRow = Record<string, unknown>

export function sqlDialect(dbType: string): SqlDialect {
  if (dbType === 'mysql' || dbType === 'postgresql') return dbType
  throw new Error(`Unsupported SQL database: ${dbType}`)
}

export function quoteIdentifier(name: string, dialect: SqlDialect): string {
  if (!name || name.includes('\0')) throw new Error('Invalid SQL identifier')
  return dialect === 'mysql'
    ? `\`${name.replace(/`/g, '``')}\``
    : `"${name.replace(/"/g, '""')}"`
}

export function sqlLiteral(value: unknown, dialect: SqlDialect): string {
  if (value === null) return 'NULL'
  if (typeof value === 'boolean') return value ? 'TRUE' : 'FALSE'
  if (typeof value === 'number') {
    if (!Number.isFinite(value) || (Number.isInteger(value) && !Number.isSafeInteger(value))) {
      throw new Error('Numeric value cannot be represented safely; use an exact string value')
    }
    return String(value)
  }
  const text = typeof value === 'string' ? value
    : typeof value === 'object' && value !== null ? JSON.stringify(value) : undefined
  if (text === undefined) throw new Error('Unsupported SQL value')
  if (dialect === 'mysql') {
    // Hex literals are independent of NO_BACKSLASH_ESCAPES and ANSI_QUOTES.
    const hex = Array.from(new TextEncoder().encode(text), byte => byte.toString(16).padStart(2, '0')).join('')
    return `CONVERT(X'${hex}' USING utf8mb4)`
  }
  if (text.includes('\0')) throw new Error('PostgreSQL text cannot contain a zero byte')
  // Explicit escape strings also work when standard_conforming_strings is off.
  return `E'${text.replace(/\\/g, '\\\\').replace(/'/g, "''")}'`
}

export function primaryKeyWhere(columns: GridColumn[], row: GridRow, dialect: SqlDialect): string {
  const keys = columns.filter(column => column.is_pk)
  if (!keys.length) throw new Error('Cannot modify a row without a primary key')
  const predicates = keys.map(({ name }) => {
    if (!Object.prototype.hasOwnProperty.call(row, name) || row[name] === null || row[name] === undefined) {
      throw new Error(`Missing primary key value: ${name}`)
    }
    return `${quoteIdentifier(name, dialect)} = ${sqlLiteral(row[name], dialect)}`
  })
  return `WHERE ${predicates.join(' AND ')}`
}

export function insertQuery(table: string, row: GridRow, dialect: SqlDialect): string {
  if (!row || typeof row !== 'object' || Array.isArray(row)) throw new Error('Expected a data object')
  const entries = Object.entries(row).filter(([, value]) => value !== undefined)
  const target = quoteIdentifier(table, dialect)
  if (!entries.length) return dialect === 'mysql'
    ? `INSERT INTO ${target} () VALUES ()`
    : `INSERT INTO ${target} DEFAULT VALUES`
  return `INSERT INTO ${target} (${entries.map(([name]) => quoteIdentifier(name, dialect)).join(', ')}) VALUES (${entries.map(([, value]) => sqlLiteral(value, dialect)).join(', ')})`
}

export function updateQuery(table: string, columns: GridColumn[], original: GridRow, values: GridRow, dialect: SqlDialect): string {
  const where = primaryKeyWhere(columns, original, dialect)
  const keys = new Set(columns.filter(column => column.is_pk).map(column => column.name))
  const assignments = Object.entries(values)
    .filter(([name, value]) => !keys.has(name) && value !== undefined)
    .map(([name, value]) => `${quoteIdentifier(name, dialect)} = ${sqlLiteral(value, dialect)}`)
  if (!assignments.length) throw new Error('No editable columns to update')
  return `UPDATE ${quoteIdentifier(table, dialect)} SET ${assignments.join(', ')} ${where}`
}

export function deleteQuery(table: string, columns: GridColumn[], row: GridRow, dialect: SqlDialect): string {
  return `DELETE FROM ${quoteIdentifier(table, dialect)} ${primaryKeyWhere(columns, row, dialect)}`
}

export function searchWhere(columns: GridColumn[], column: string | null, keyword: string, dialect: SqlDialect): string {
  if (!keyword.trim()) return ''
  const selected = column && column !== '__all__' ? columns.filter(item => item.name === column) : columns
  if (!selected.length) return ''
  const pattern = sqlLiteral(`%${keyword.trim()}%`, dialect)
  const type = dialect === 'mysql' ? 'CHAR' : 'TEXT'
  return ` WHERE (${selected.map(item => `CAST(${quoteIdentifier(item.name, dialect)} AS ${type}) LIKE ${pattern}`).join(' OR ')})`
}

/** Parse complete CSV records, including quoted newlines, without trimming data. */
export function parseCSV(text: string): Record<string, string>[] {
  const records: string[][] = []
  let fields: string[] = []
  let field = ''
  let inQuotes = false
  let afterQuote = false
  let started = false
  const input = text.replace(/^\uFEFF/, '')
  const finishField = () => { fields.push(field); field = ''; afterQuote = false }
  const finishRecord = () => {
    if (started || field.length || fields.length) {
      finishField()
      records.push(fields)
    }
    fields = []; field = ''; started = false; afterQuote = false
  }
  for (let i = 0; i < input.length; i++) {
    const char = input[i]!
    if (inQuotes) {
      if (char === '"') {
        if (input[i + 1] === '"') { field += '"'; i++ }
        else { inQuotes = false; afterQuote = true }
      } else field += char
      continue
    }
    if (char === ',') { finishField(); started = true }
    else if (char === '\n' || char === '\r') {
      finishRecord()
      if (char === '\r' && input[i + 1] === '\n') i++
    } else if (char === '"' && !field && !afterQuote) {
      inQuotes = true; started = true
    } else {
      if (afterQuote || char === '"') throw new Error('Malformed CSV quoted field')
      field += char; started = true
    }
  }
  if (inQuotes) throw new Error('Unterminated CSV quoted field')
  finishRecord()
  const headers = records.shift()
  if (!headers) return []
  if (headers.some(header => !header) || new Set(headers).size !== headers.length) {
    throw new Error('CSV column names must be nonempty and unique')
  }
  return records.map((values, index) => {
    if (values.length !== headers.length) throw new Error(`CSV record ${index + 2} has an incorrect column count`)
    return Object.fromEntries(headers.map((header, index) => [header, values[index]!]))
  })
}
