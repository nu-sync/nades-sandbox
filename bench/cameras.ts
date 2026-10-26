/*
 * The web view's cameras for a list of throws, from its own code: show-me-the-nades' web/app/src/scene/stage.ts
 * (buildStage, resultCamera, flightOf) read in place, fed by its API (/api/scene, /api/viewers) and awpy's collision
 * mesh in a three-mesh-bvh tree, as render/mapModel.ts builds the collider. One JSON line per throw, the shape
 * `nades-sandbox --cameras` prints, for bench/cameras.py to compare.
 *
 *   bun bench/cameras.ts --web <show-me-the-nades>/web/app --api http://localhost:8899 --cases cases.json [--maps DIR]
 */
import { readFileSync } from "node:fs"
import { homedir } from "node:os"
import { join } from "node:path"

const args = process.argv.slice(2)
const option = (name: string, fallback?: string): string => {
  const i = args.indexOf(`--${name}`)
  const value = i >= 0 ? args[i + 1] : fallback
  if (value === undefined) throw new Error(`--${name} is needed`)
  return value
}
const app = option("web")
const api = option("api").replace(/\/$/, "")
const maps = option("maps", join(homedir(), ".cache", "nades", "maps"))
const cases = JSON.parse(readFileSync(option("cases"), "utf8")) as { throw: string; map: string }[]

// what the 3D modules touch of a browser, and do not need here: a flash's burst draws its glow on a canvas
;(globalThis as { document?: unknown }).document ??= { createElement: () => ({ width: 0, height: 0, getContext: () => null }) }

// the web app's own copies of three and three-mesh-bvh, so the stage's modules and this script share them
const resolve = (name: string) => Bun.resolveSync(name, join(app, "src"))
const THREE = (await import(resolve("three"))) as typeof import("three")
const { MeshBVH } = (await import(resolve("three-mesh-bvh"))) as typeof import("three-mesh-bvh")
const { buildStage, resultCamera, flightOf } = await import(join(app, "src/scene/stage.ts"))
const { anglesTo } = await import(join(app, "src/scene/camera.ts"))

/** awpy's mesh (AWMH) as the web view reads it (render/mapModel.ts collisionCollider), its tree as colliderTree builds it. */
const colliders = new Map<string, unknown>()
const collider = (map: string) => {
  if (colliders.has(map)) return colliders.get(map)
  let bvh: unknown = null
  try {
    const bytes = readFileSync(join(maps, `${map}.mesh`))
    const buffer = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength)
    const head = new DataView(buffer)
    if (head.getUint32(0, false) === 0x41574d48) {
      const vertices = head.getUint32(8, true)
      const triangles = head.getUint32(12, true)
      const geometry = new THREE.BufferGeometry()
      geometry.setAttribute("position", new THREE.BufferAttribute(new Float32Array(buffer.slice(16, 16 + vertices * 12)), 3))
      geometry.setIndex(new THREE.BufferAttribute(new Uint32Array(buffer.slice(16 + vertices * 12, 16 + vertices * 12 + triangles * 12)), 1))
      bvh = new MeshBVH(geometry, { targetLeafSize: 32 })
    }
  } catch {
    bvh = null
  }
  colliders.set(map, bvh)
  return bvh
}

const rows = async (path: string) => {
  const response = await fetch(`${api}${path}`)
  if (!response.ok) throw new Error(`${path}: HTTP ${response.status}`)
  return ((await response.json()) as { rows: unknown[] }).rows
}

type V = readonly [number, number, number]
const camera = (v: { eye: V; pitch: number; yaw: number; fov?: number }) => ({ eye: [...v.eye], pitch: v.pitch, yaw: v.yaw, fov: v.fov ?? null })

for (const c of cases) {
  const [scene] = (await rows(`/api/scene?id=${encodeURIComponent(c.throw)}`)) as Record<string, unknown>[]
  if (!scene) continue
  const viewers = await rows(`/api/viewers?id=${encodeURIComponent(c.throw)}`)
  const bvh = collider(scene.map as string)
  const model = bvh ? { collider: bvh } : null
  const started = performance.now()
  const stage = buildStage(scene)
  const result = resultCamera(stage, viewers, model)
  const ms = performance.now() - started
  // the orbit opens on its move's first pose: the thrower's view at release
  const first = flightOf(scene, stage, model).pose(0)
  const orbit = { eye: first.eye, ...anglesTo(first.eye, first.target) }
  console.log(
    JSON.stringify({
      throw: scene.throw_id,
      map: scene.map,
      nade: scene.nade,
      lineup: camera(stage.lineup),
      aim: camera(stage.aim),
      result: { ...camera(result), fov: stage.lineup.fov, kind: result.source.kind, player: result.source.player ?? null, tick: result.source.tick ?? null, seen: result.seen ?? null },
      orbit: camera({ ...orbit, fov: stage.lineup.fov }),
      focus: [stage.focus.x, stage.focus.y, stage.focus.z],
      result_time: stage.resultTime,
      duration: stage.duration,
      viewers: viewers.length,
      missing: stage.missing,
      collider: bvh !== null,
      ms,
    }),
  )
}
// the 3D modules leave timers behind
process.exit(0)
