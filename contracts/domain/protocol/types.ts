// Generated from contracts/domain/protocol/*.schema.json. Do not edit.
export type Finished = ({ "kind": "json"; "value": unknown; } | { "kind": "stream"; "value": Output; });
export type Open = { "request": unknown; "stream": boolean; };
export type Opened = ({ "kind": "json"; "value": unknown; } | { "kind": "bytes"; "value": Array<number>; });
export type Output = { "chunks": Array<Array<number>>; "complete": boolean; };
