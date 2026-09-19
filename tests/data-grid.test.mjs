import test from 'node:test'
import assert from 'node:assert/strict'
import {
  sqlDialect, quoteIdentifier, sqlLiteral, primaryKeyWhere,
  insertQuery, updateQuery, deleteQuery, searchWhere, parseCSV
} from '../src/utils/dataGrid.ts'

const columns = [{ name: 'tenant', is_pk: true }, { name: 'id', is_pk: true }, { name: 'value' }]
for (const dialect of ['mysql', 'postgresql']) {
  test(`${dialect}: mutations constrain every component of the original primary key`, () => {
    const original = { tenant: 7, id: 42, value: 'before' }
    const where = primaryKeyWhere(columns, original, dialect)
    assert.equal(where, `WHERE ${quoteIdentifier('tenant', dialect)} = 7 AND ${quoteIdentifier('id', dialect)} = 42`)
    assert.equal(deleteQuery('order', columns, original, dialect), `DELETE FROM ${quoteIdentifier('order', dialect)} ${where}`)
    const updated = updateQuery('order', columns, original, { tenant: 8, id: 99, value: 'after' }, dialect)
    assert.ok(updated.endsWith(where))
    assert.equal(updated, `UPDATE ${quoteIdentifier('order', dialect)} SET ${quoteIdentifier('value', dialect)} = ${sqlLiteral('after', dialect)} ${where}`)
  })
  test(`${dialect}: missing, null, and incomplete keys fail closed; zero keys work`, () => {
    for (const row of [{ tenant: 7 }, { tenant: 7, id: null }, { tenant: 7, id: undefined }]) {
      assert.throws(() => deleteQuery('t', columns, row, dialect), /Missing primary key/)
    }
    assert.throws(() => deleteQuery('t', [], {}, dialect), /without a primary key/)
    assert.ok(primaryKeyWhere(columns, { tenant: 0, id: 0 }, dialect).endsWith(' = 0'))
  })
  test(`${dialect}: inserts and exports preserve null, empty text, false and JSON`, () => {
    const row = { empty: '', nullable: null, enabled: false, document: { x: "O'Reilly" } }
    assert.equal(insertQuery('t', row, dialect), `INSERT INTO ${quoteIdentifier('t', dialect)} (${Object.keys(row).map(k => quoteIdentifier(k, dialect)).join(', ')}) VALUES (${Object.values(row).map(v => sqlLiteral(v, dialect)).join(', ')})`)
    assert.ok(insertQuery('t', row, dialect).includes('NULL, FALSE'))
  })
  test(`${dialect}: invalid numeric values and unsupported data are rejected`, () => {
    for (const value of [NaN, Infinity, -Infinity, Number.MAX_SAFE_INTEGER + 1, undefined]) {
      assert.throws(() => sqlLiteral(value, dialect))
    }
    assert.throws(() => insertQuery('t', null, dialect), /data object/)
    assert.throws(() => insertQuery('t', [], dialect), /data object/)
    assert.throws(() => updateQuery('t', columns, { tenant: 7, id: 42 }, { id: 10 }, dialect), /No editable/)
  })
  test(`${dialect}: all-column search casts numeric and JSON columns to text`, () => {
    const where = searchWhere(columns, null, '42', dialect)
    assert.equal((where.match(/CAST\(/g) || []).length, 3)
    assert.ok(where.includes(dialect === 'mysql' ? ' AS CHAR)' : ' AS TEXT)'))
    assert.equal(searchWhere(columns, null, '  ', dialect), '')
    assert.equal(searchWhere(columns, 'absent', 'x', dialect), '')
    assert.equal((searchWhere(columns, 'id', 'x', dialect).match(/CAST\(/g) || []).length, 1)
  })
}

test('SQL identifiers preserve literal dots, spaces, reserved words and escaped delimiters', () => {
  assert.equal(quoteIdentifier('schema.table', 'postgresql'), '"schema.table"')
  assert.equal(quoteIdentifier('my`table', 'mysql'), '`my``table`')
  assert.equal(quoteIdentifier('my"table', 'postgresql'), '"my""table"')
  assert.equal(quoteIdentifier('order', 'mysql'), '`order`')
  assert.throws(() => quoteIdentifier('', 'mysql'))
  assert.throws(() => quoteIdentifier('bad\0name', 'postgresql'))
  assert.throws(() => sqlDialect('redis'))
})

test('MySQL strings use UTF-8 hex regardless of SQL escape mode', () => {
  for (const value of ["x' OR 1=1 --", "\\'; DROP TABLE t; --", '中文😀', 'line\nnext', '', '\0', '%_']) {
    const actual = sqlLiteral(value, 'mysql')
    assert.equal(actual, `CONVERT(X'${Buffer.from(value, 'utf8').toString('hex')}' USING utf8mb4)`)
  }
})

test('PostgreSQL strings escape quotes and backslashes explicitly', () => {
  assert.equal(sqlLiteral("O'Reilly", 'postgresql'), "E'O''Reilly'")
  assert.equal(sqlLiteral('a\\b', 'postgresql'), "E'a\\\\b'")
  assert.equal(sqlLiteral("\\'; DELETE FROM t; --", 'postgresql'), "E'\\\\''; DELETE FROM t; --'")
  assert.throws(() => sqlLiteral('zero\0byte', 'postgresql'), /zero byte/)
})

test('empty inserts use dialect-specific default syntax', () => {
  assert.equal(insertQuery('t', {}, 'mysql'), 'INSERT INTO `t` () VALUES ()')
  assert.equal(insertQuery('t', {}, 'postgresql'), 'INSERT INTO "t" DEFAULT VALUES')
})

test('CSV handles BOM, CRLF, commas, doubled quotes and embedded newlines', () => {
  assert.deepEqual(parseCSV('\uFEFF"id","body"\r\n"1","first\r\nsecond, ""quoted"""\r\n'), [
    { id: '1', body: 'first\r\nsecond, "quoted"' }
  ])
})

test('CSV preserves leading/trailing whitespace, empty strings and blank lines inside fields', () => {
  assert.deepEqual(parseCSV('a,b\n"  x  ",""\n\n"\n\n", y '), [
    { a: '  x  ', b: '' }, { a: '\n\n', b: ' y ' }
  ])
  assert.deepEqual(parseCSV('a\n""\n'), [{ a: '' }])
  assert.deepEqual(parseCSV('a,b\n,\n'), [{ a: '', b: '' }])
})

test('CSV rejects malformed records rather than silently corrupting imported data', () => {
  for (const csv of ['a,b\n"unfinished,x', 'a,b\n1', 'a,b\n1,2,3', 'a,a\n1,2', 'a,\n1,2', 'a\n"x"bad']) {
    assert.throws(() => parseCSV(csv))
  }
  assert.deepEqual(parseCSV(''), [])
  assert.deepEqual(parseCSV('a,b\n'), [])
})

test('CSV treats __proto__ as a data column without mutating object prototypes', () => {
  const [row] = parseCSV('__proto__,value\nsafe,ok')
  assert.equal(Object.getPrototypeOf(row), Object.prototype)
  assert.equal(row.__proto__, 'safe')
})

for (const dialect of ['mysql', 'postgresql']) {
  test(`${dialect}: binary values remain bytes in writes and exports`, () => {
    const columns = [{ name: 'id', is_pk: true, type_name: dialect === 'mysql' ? 'varbinary(16)' : 'bytea' }, { name: 'payload', type_name: dialect === 'mysql' ? 'blob' : 'bytea' }]
    const query = insertQuery('t', { id: '0x0001', payload: '0xABCD' }, dialect, columns)
    assert.ok(query.includes(dialect === 'mysql' ? "X'ABCD'" : "decode('ABCD', 'hex')"))
    assert.ok(deleteQuery('t', columns, { id: '0x0001' }, dialect).includes(dialect === 'mysql' ? "X'0001'" : "decode('0001', 'hex')"))
    assert.throws(() => insertQuery('t', { payload: '0xAB...' }, dialect, columns), /complete/)
    assert.throws(() => insertQuery('t', { payload: '0xA' }, dialect, columns), /complete/)
  })
}

test('JSON import rejects precision loss including nested decimals', async () => {
  const { parseImportJSON } = await import('../src/utils/dataGrid.ts')
  for (const source of ['[{"id":9007199254740993}]', '[{"json":{"n":1234.1234567890123456789}}]', '[{"n":1e400}]', '[{"n":1e-400}]']) {
    assert.throws(() => parseImportJSON(source), /precision/)
  }
  assert.deepEqual(parseImportJSON('[{"amount":"1234.1234567890123456789","n":1.2300e2,"small":0.1}]'), [{ amount: '1234.1234567890123456789', n: 123, small: 0.1 }])
})
