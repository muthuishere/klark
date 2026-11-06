import asyncio, gzip, json, sys, time
from websockets.asyncio.server import serve
stats = {"conn":0,"tree":0,"unchanged":0,"frames":0,"frame_bytes":0,"wire_bytes":0,"results":0,"bad_auth":0}
async def handler(ws):
    if ws.request.headers.get("Authorization") != "Bearer fleet":
        stats["bad_auth"] += 1; await ws.close(); return
    stats["conn"] += 1
    mid = ws.request.headers.get("X-Klark-Machine")
    rev = None
    async def puller():
        nonlocal rev
        n = 0
        while True:
            await ws.send(json.dumps({"type":"get_tree","known_rev":rev}))
            if n == 1 and int(mid[-3:]) % int(__import__("os").environ.get("EVERY","4")) == 0:
                await ws.send(json.dumps({"type":"subscribe","pane_id":"p0"}))
                await ws.send(json.dumps({"type":"call","id":"1","method":"pane.read","params":{}}))
            n += 1
            await asyncio.sleep(2)
    t = asyncio.create_task(puller())
    try:
        async for raw in ws:
            stats["wire_bytes"] += len(raw)
            if isinstance(raw, bytes) and raw[0] in (1, 2):
                n = raw[1]; body = raw[2+n:]
                data = gzip.decompress(body) if raw[0] == 1 else body
                stats["frames"] += 1; stats["frame_bytes"] += len(data); continue
            m = json.loads(gzip.decompress(raw)) if isinstance(raw, bytes) else json.loads(raw)
            ty = m["type"]
            if ty == "tree": stats["tree"] += 1; rev = m["rev"]
            elif ty == "tree_unchanged": stats["unchanged"] += 1
            elif ty == "frame": stats["frames"] += 1; stats["frame_bytes"] += len(m["bytes"])*3//4
            elif ty == "result": stats["results"] += 1
    finally:
        t.cancel()
async def main():
    async with serve(handler, "127.0.0.1", 18080, compression=None):
        await asyncio.sleep(float(sys.argv[1]))
    print(json.dumps(stats))
asyncio.run(main())
