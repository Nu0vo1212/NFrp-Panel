/**
 * 图标统一出口：基于 **Morphicons**（morphicons/vue）+ **Lucide** 图标数据。
 *
 * 规范：
 * - 全站只用这一处的图标，**不用 emoji、不手绘 SVG**；
 * - 图标即数据（Lucide 的 `IconNode`），由 MorphIcon 渲染；
 * - 状态切换时（如 播放/停止、显示/隐藏密码）会走 Morphicons 的弹簧变形；
 * - `reducedMotion="user"`：尊重系统"减少动效"偏好。
 */
import { MorphIcon, type MorphHandle } from 'morphicons/vue'
import {
  Activity,
  ArrowLeft,
  ArrowRight,
  Check,
  ChevronLeft,
  ChevronRight,
  CircleCheck,
  Cpu,
  Download,
  Eye,
  EyeOff,
  FileCode,
  Fingerprint,
  Gauge,
  KeyRound,
  Layers,
  LoaderCircle,
  LogOut,
  Moon,
  Network,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  RotateCw,
  Save,
  ScrollText,
  Server,
  Settings,
  ShieldCheck,
  Square,
  Sun,
  Terminal,
  Trash2,
  TriangleAlert,
  Users,
  Waypoints,
  X,
  type IconNode,
} from 'lucide'

/** 语义化图标表：页面只引用这里的名字，便于统一替换。 */
export const icons = {
  // 导航
  server: Server,
  audit: ScrollText,
  settings: Settings,
  users: Users,

  // 生命周期
  play: Play,
  stop: Square,
  restart: RotateCw,
  refresh: RefreshCw,
  loading: LoaderCircle,

  // 操作
  plus: Plus,
  edit: Pencil,
  delete: Trash2,
  save: Save,
  download: Download,
  back: ArrowLeft,
  forward: ArrowRight,
  chevronLeft: ChevronLeft,
  chevronRight: ChevronRight,
  close: X,
  check: Check,

  // 状态
  ok: CircleCheck,
  warning: TriangleAlert,

  // 领域
  file: FileCode,
  terminal: Terminal,
  activity: Activity,
  gauge: Gauge,
  layers: Layers,
  network: Network,
  cpu: Cpu,
  shield: ShieldCheck,
  key: KeyRound,
  fingerprint: Fingerprint,
  waypoints: Waypoints,

  // 主题与账号
  sun: Sun,
  moon: Moon,
  logout: LogOut,
  eye: Eye,
  eyeOff: EyeOff,
} as const satisfies Record<string, IconNode>

export type IconName = keyof typeof icons

export { MorphIcon }
export type { MorphHandle, IconNode }
