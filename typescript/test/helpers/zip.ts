const crcTable = Uint32Array.from({ length: 256 }, (_, index) => {
  let value = index;
  for (let bit = 0; bit < 8; bit += 1) {
    value = value & 1 ? 0xedb88320 ^ (value >>> 1) : value >>> 1;
  }
  return value >>> 0;
});

const crc32 = (bytes: Uint8Array): number =>
  (bytes.reduce((crc, byte) => (crcTable[(crc ^ byte) & 0xff] ?? 0) ^ (crc >>> 8), 0xffffffff) ^
    0xffffffff) >>>
  0;

export interface ZipMember {
  readonly name: string;
  readonly data: Uint8Array | null;
}

const UNIX = 3 << 8;
const FILE_MODE = (0o100644 << 16) >>> 0;
const DIRECTORY_MODE = (0o040755 << 16) >>> 0;

const header = (fields: readonly (readonly [number, number])[]): Buffer => {
  const size = fields.reduce((total, [width]) => total + width, 0);
  const buffer = Buffer.alloc(size);
  fields.reduce((offset, [width, value]) => {
    if (width === 2) {
      buffer.writeUInt16LE(value, offset);
    } else {
      buffer.writeUInt32LE(value, offset);
    }
    return offset + width;
  }, 0);
  return buffer;
};

export function storedZip(members: readonly ZipMember[]): Buffer {
  const records = members.map((member) => {
    const name = Buffer.from(member.data === null ? `${member.name}/` : member.name, 'utf8');
    const data = Buffer.from(member.data ?? new Uint8Array());
    const flags = name.every((byte) => byte < 0x80) ? 0 : 0x0800;
    return { name, data, flags, crc: crc32(data), directory: member.data === null };
  });
  const offsets = records.reduce<number[]>(
    (all, record) => [...all, (all.at(-1) ?? 0) + 30 + record.name.length + record.data.length],
    [],
  );
  const local = records.map((record) =>
    Buffer.concat([
      header([
        [4, 0x04034b50],
        [2, 20],
        [2, record.flags],
        [2, 0],
        [2, 0],
        [2, 0x21],
        [4, record.crc],
        [4, record.data.length],
        [4, record.data.length],
        [2, record.name.length],
        [2, 0],
      ]),
      record.name,
      record.data,
    ]),
  );
  const central = records.map((record, index) =>
    Buffer.concat([
      header([
        [4, 0x02014b50],
        [2, UNIX],
        [2, 20],
        [2, record.flags],
        [2, 0],
        [2, 0],
        [2, 0x21],
        [4, record.crc],
        [4, record.data.length],
        [4, record.data.length],
        [2, record.name.length],
        [2, 0],
        [2, 0],
        [2, 0],
        [2, 0],
        [4, record.directory ? DIRECTORY_MODE : FILE_MODE],
        [4, index === 0 ? 0 : (offsets[index - 1] ?? 0)],
      ]),
      record.name,
    ]),
  );
  const body = Buffer.concat(local);
  const directory = Buffer.concat(central);
  return Buffer.concat([
    body,
    directory,
    header([
      [4, 0x06054b50],
      [2, 0],
      [2, 0],
      [2, records.length],
      [2, records.length],
      [4, directory.length],
      [4, body.length],
      [2, 0],
    ]),
  ]);
}
