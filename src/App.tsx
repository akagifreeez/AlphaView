import { useState, useEffect, useRef, useCallback, useMemo, memo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import "./App.css";

interface RawInfo {
  width: number;
  height: number;
  make: string;
  model: string;
  clean_make: string;
  clean_model: string;
  iso: string | null;
  shutter_speed: string | null;
  aperture: string | null;
  focal_length: string | null;
  lens_model: string | null;
  date_taken: string | null;
  file_size_bytes: number;
  thumbnail_base64: string | null;
}

function formatFileSize(bytes: number): string {
  if (bytes >= 1_000_000_000) return (bytes / 1_000_000_000).toFixed(1) + " GB";
  if (bytes >= 1_000_000) return (bytes / 1_000_000).toFixed(1) + " MB";
  if (bytes >= 1_000) return (bytes / 1_000).toFixed(1) + " KB";
  return bytes + " B";
}

/* ════════ Centralised fetch queue ════════ */
const thumbnailCache = new Map<string, RawInfo>();
const listeners = new Map<string, Set<() => void>>();
type QueueItem = { path: string; priority: boolean };
let queue: QueueItem[] = [];
let activeCount = 0;
const MAX_CONCURRENT = 4;

function enqueue(path: string, priority: boolean) {
  if (thumbnailCache.has(path)) return;
  if (queue.some((q) => q.path === path)) {
    if (priority) { queue = queue.filter((q) => q.path !== path); queue.unshift({ path, priority: true }); }
    return;
  }
  priority ? queue.unshift({ path, priority: true }) : queue.push({ path, priority: false });
  processQueue();
}
async function processQueue() {
  while (activeCount < MAX_CONCURRENT && queue.length > 0) {
    const item = queue.shift()!;
    if (thumbnailCache.has(item.path)) continue;
    activeCount++;
    invoke<RawInfo>("load_raw_info", { path: item.path })
      .then((info) => { thumbnailCache.set(item.path, info); listeners.get(item.path)?.forEach((cb) => cb()); })
      .catch(() => { })
      .finally(() => { activeCount--; processQueue(); });
  }
}
function subscribe(path: string, cb: () => void) {
  if (!listeners.has(path)) listeners.set(path, new Set());
  listeners.get(path)!.add(cb);
  return () => { listeners.get(path)?.delete(cb); };
}
function schedulePrefetch(files: string[]) { for (const f of files) enqueue(f, false); }

/* ════════ useContainerSize ════════ */
function useContainerSize(ref: React.RefObject<HTMLDivElement | null>) {
  const [size, setSize] = useState({ width: 0, height: 0 });
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(([e]) => { setSize({ width: Math.floor(e.contentRect.width), height: Math.floor(e.contentRect.height) }); });
    ro.observe(el);
    setSize({ width: Math.floor(el.clientWidth), height: Math.floor(el.clientHeight) });
    return () => ro.disconnect();
  }, [ref]);
  return size;
}

/* ════════ ThumbnailCard ════════ */
const ThumbnailCard = memo(({ path, selected, onClick }: { path: string; selected: boolean; onClick: (p: string) => void }) => {
  const [rawInfo, setRawInfo] = useState<RawInfo | null>(thumbnailCache.get(path) || null);
  useEffect(() => {
    const cached = thumbnailCache.get(path);
    if (cached) { setRawInfo(cached); return; }
    setRawInfo(null);
    const unsub = subscribe(path, () => { const info = thumbnailCache.get(path); if (info) setRawInfo(info); });
    enqueue(path, true);
    return unsub;
  }, [path]);
  const filename = path.split(/[\\/]/).pop();
  return (
    <div className={`thumb-card${selected ? " selected" : ""}`} onClick={() => onClick(path)}>
      {rawInfo?.thumbnail_base64 ? (
        <img src={rawInfo.thumbnail_base64} alt={filename} />
      ) : (
        <div className="placeholder">Loading...</div>
      )}
      <div className="filename">{filename}</div>
      {rawInfo && <div className="meta">{rawInfo.model} · {rawInfo.width}x{rawInfo.height}</div>}
    </div>
  );
});

/* ════════ DevelopParams type ════════ */
interface DevelopParams {
  exposure: number;
  saturation: number;
  contrast: number;
  highlights: number;
  shadows: number;
  wb_temp_shift: number;
  wb_tint_shift: number;
}

const DEFAULT_DEVELOP: DevelopParams = {
  exposure: 0,
  saturation: 1.3,
  contrast: 0,
  highlights: 0,
  shadows: 0,
  wb_temp_shift: 0,
  wb_tint_shift: 0,
};

/* ════════ Slider row component ════════ */
function DevSlider({ label, value, min, max, step, onChange }: {
  label: string; value: number; min: number; max: number; step: number;
  onChange: (v: number) => void;
}) {
  return (
    <div className="dev-slider-row">
      <label className="dev-slider-label">{label}</label>
      <input
        type="range" min={min} max={max} step={step} value={value}
        onChange={(e) => onChange(parseFloat(e.target.value))}
        className="dev-slider-input"
      />
      <span className="dev-slider-value">{value.toFixed(2)}</span>
    </div>
  );
}

/* ════════ PreviewOverlay ════════ */
function PreviewOverlay({ path, onClose, onPrev, onNext }: { path: string; onClose: () => void; onPrev: () => void; onNext: () => void }) {
  const rawInfo = thumbnailCache.get(path);
  const filename = path.split(/[\\/]/).pop();
  const [decodedSrc, setDecodedSrc] = useState<string | null>(null);
  const [decoding, setDecoding] = useState(false);
  const [decodeError, setDecodeError] = useState<string | null>(null);
  const [develop, setDevelop] = useState<DevelopParams>({ ...DEFAULT_DEVELOP });
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const cancelRef = useRef(false);

  // Decode function with current develop params
  const doDecode = useCallback((devParams: DevelopParams, immediate?: boolean) => {
    if (debounceRef.current) clearTimeout(debounceRef.current);

    const run = () => {
      cancelRef.current = true; // cancel previous
      cancelRef.current = false;
      setDecoding(true);
      setDecodeError(null);
      const thisCancel = cancelRef;

      invoke<string>("decode_raw_full", {
        path,
        maxDimension: 2048,
        develop: devParams,
      })
        .then((src) => { if (!thisCancel.current) setDecodedSrc(src); })
        .catch((e) => { if (!thisCancel.current) setDecodeError(String(e)); })
        .finally(() => { if (!thisCancel.current) setDecoding(false); });
    };

    if (immediate) {
      run();
    } else {
      debounceRef.current = setTimeout(run, 300);
    }
  }, [path]);

  // Initial decode when path changes
  useEffect(() => {
    setDecodedSrc(null);
    setDevelop({ ...DEFAULT_DEVELOP });
    doDecode(DEFAULT_DEVELOP, true);
    return () => { cancelRef.current = true; };
  }, [path, doDecode]);

  // Handle slider change
  const handleSliderChange = useCallback((key: keyof DevelopParams, value: number) => {
    setDevelop((prev) => {
      const next = { ...prev, [key]: value };
      doDecode(next);
      return next;
    });
  }, [doDecode]);

  // Reset all
  const handleReset = useCallback(() => {
    setDevelop({ ...DEFAULT_DEVELOP });
    doDecode(DEFAULT_DEVELOP, true);
  }, [doDecode]);

  useEffect(() => {
    const h = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); if (e.key === "ArrowLeft") onPrev(); if (e.key === "ArrowRight") onNext(); };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [onClose, onPrev, onNext]);

  const imgSrc = decodedSrc || rawInfo?.thumbnail_base64;
  const isRawDecoded = !!decodedSrc;

  return (
    <div className="preview-overlay" onClick={onClose}>
      <div className="preview-layout" onClick={(e) => e.stopPropagation()}>
        {/* Image area */}
        <div className="preview-image-area">
          {imgSrc ? (
            <div style={{ position: "relative" }}>
              <img src={imgSrc} alt={filename} />
              {decoding && (
                <div style={{ position: "absolute", top: 8, right: 8, background: "rgba(0,0,0,0.7)", color: "#fff", padding: "4px 10px", borderRadius: 4, fontSize: 12 }}>
                  ⏳ RAW Decoding...
                </div>
              )}
              {isRawDecoded && !decoding && (
                <div style={{ position: "absolute", top: 8, right: 8, background: "rgba(52,120,246,0.9)", color: "#fff", padding: "4px 10px", borderRadius: 4, fontSize: 11, fontWeight: "bold" }}>
                  RAW
                </div>
              )}
            </div>
          ) : (
            <div style={{ color: "#888", fontSize: 18 }}>No preview</div>
          )}
          {decodeError && (
            <div style={{ color: "#ff6b6b", fontSize: 12, marginTop: 8 }}>
              ⚠ RAW decode failed: {decodeError}
            </div>
          )}
          <div className="preview-info">
            <strong>{filename}</strong>
            {rawInfo && <span className="detail">{rawInfo.make} {rawInfo.model} · {rawInfo.width}x{rawInfo.height}</span>}
          </div>
          <div className="preview-controls">
            <button onClick={(e) => { e.stopPropagation(); onPrev(); }}>◀ Prev</button>
            <button className="close-btn" onClick={(e) => { e.stopPropagation(); onClose(); }}>✕ Close</button>
            <button onClick={(e) => { e.stopPropagation(); onNext(); }}>Next ▶</button>
          </div>
        </div>

        {/* Develop panel */}
        <div className="develop-panel">
          <div className="develop-header">
            <span>現像設定</span>
            <button className="dev-reset-btn" onClick={handleReset}>Reset</button>
          </div>
          <div className="develop-section-title">Light</div>
          <DevSlider label="Exposure" value={develop.exposure} min={-3} max={3} step={0.1} onChange={(v) => handleSliderChange("exposure", v)} />
          <DevSlider label="Contrast" value={develop.contrast} min={-1} max={1} step={0.05} onChange={(v) => handleSliderChange("contrast", v)} />
          <DevSlider label="Highlights" value={develop.highlights} min={-1} max={1} step={0.05} onChange={(v) => handleSliderChange("highlights", v)} />
          <DevSlider label="Shadows" value={develop.shadows} min={-1} max={1} step={0.05} onChange={(v) => handleSliderChange("shadows", v)} />

          <div className="develop-section-title">Color</div>
          <DevSlider label="Saturation" value={develop.saturation} min={0} max={2} step={0.05} onChange={(v) => handleSliderChange("saturation", v)} />
          <DevSlider label="WB Temp" value={develop.wb_temp_shift} min={-1} max={1} step={0.05} onChange={(v) => handleSliderChange("wb_temp_shift", v)} />
          <DevSlider label="WB Tint" value={develop.wb_tint_shift} min={-1} max={1} step={0.05} onChange={(v) => handleSliderChange("wb_tint_shift", v)} />
        </div>
      </div>
    </div>
  );
}

/* ════════ VirtualGrid ════════ */
function VirtualGrid({ items, containerWidth, containerHeight, colWidth, rowHeight, selectedPath, onClickItem }: {
  items: string[]; containerWidth: number; containerHeight: number;
  colWidth: number; rowHeight: number; selectedPath: string | null; onClickItem: (p: string) => void;
}) {
  const [scrollTop, setScrollTop] = useState(0);
  const scrollRef = useRef<HTMLDivElement>(null);
  const cols = Math.max(1, Math.floor(containerWidth / colWidth));
  const totalRows = Math.ceil(items.length / cols);
  const totalHeight = totalRows * rowHeight;
  const overscan = 3;
  const startRow = Math.max(0, Math.floor(scrollTop / rowHeight) - overscan);
  const endRow = Math.min(totalRows, startRow + Math.ceil(containerHeight / rowHeight) + overscan * 2);
  const visibleItems = useMemo(() => {
    const r: { path: string; row: number; col: number }[] = [];
    for (let row = startRow; row < endRow; row++)
      for (let col = 0; col < cols; col++) { const i = row * cols + col; if (i < items.length) r.push({ path: items[i], row, col }); }
    return r;
  }, [items, startRow, endRow, cols]);
  useEffect(() => { schedulePrefetch(items); }, [items]);
  const handleScroll = useCallback(() => { if (!scrollRef.current) return; requestAnimationFrame(() => { if (scrollRef.current) setScrollTop(scrollRef.current.scrollTop); }); }, []);
  return (
    <div ref={scrollRef} onScroll={handleScroll} style={{ width: "100%", height: "100%", overflow: "auto" }}>
      <div style={{ position: "relative", width: cols * colWidth, height: totalHeight }}>
        {visibleItems.map(({ path, row, col }) => (
          <div key={path} style={{ position: "absolute", top: row * rowHeight, left: col * colWidth, width: colWidth, height: rowHeight, padding: "6px", boxSizing: "border-box" }}>
            <ThumbnailCard path={path} selected={path === selectedPath} onClick={onClickItem} />
          </div>
        ))}
      </div>
    </div>
  );
}

/* ════════ MetadataPanel ════════ */
function MetadataPanel({ path }: { path: string | null }) {
  const rawInfo = path ? thumbnailCache.get(path) : null;
  const filename = path?.split(/[\\/]/).pop();
  if (!path) return (
    <aside className="metadata-panel">
      <h3>Info</h3>
      <div className="sidebar-info">Select an image to view details</div>
    </aside>
  );
  return (
    <aside className="metadata-panel">
      {rawInfo?.thumbnail_base64 && <img className="metadata-thumb" src={rawInfo.thumbnail_base64} alt={filename} />}

      <h3>File</h3>
      <div className="metadata-row"><span className="label">Name</span><span className="value">{filename}</span></div>
      {rawInfo && <div className="metadata-row"><span className="label">Size</span><span className="value">{formatFileSize(rawInfo.file_size_bytes)}</span></div>}
      {rawInfo?.date_taken && <div className="metadata-row"><span className="label">Date</span><span className="value">{rawInfo.date_taken}</span></div>}

      {rawInfo && (<>
        <h3 style={{ marginTop: "16px" }}>Camera</h3>
        <div className="metadata-row"><span className="label">Make</span><span className="value">{rawInfo.make}</span></div>
        <div className="metadata-row"><span className="label">Model</span><span className="value">{rawInfo.model}</span></div>
        {rawInfo.lens_model && <div className="metadata-row"><span className="label">Lens</span><span className="value">{rawInfo.lens_model}</span></div>}

        <h3 style={{ marginTop: "16px" }}>Shooting</h3>
        {rawInfo.shutter_speed && <div className="metadata-row"><span className="label">Shutter</span><span className="value">{rawInfo.shutter_speed}</span></div>}
        {rawInfo.aperture && <div className="metadata-row"><span className="label">Aperture</span><span className="value">f/{rawInfo.aperture}</span></div>}
        {rawInfo.iso && <div className="metadata-row"><span className="label">ISO</span><span className="value">{rawInfo.iso}</span></div>}
        {rawInfo.focal_length && <div className="metadata-row"><span className="label">Focal</span><span className="value">{rawInfo.focal_length}</span></div>}

        <h3 style={{ marginTop: "16px" }}>Image</h3>
        <div className="metadata-row"><span className="label">Resolution</span><span className="value">{rawInfo.width} × {rawInfo.height}</span></div>
        <div className="metadata-row"><span className="label">Megapixels</span><span className="value">{((rawInfo.width * rawInfo.height) / 1_000_000).toFixed(1)} MP</span></div>
      </>)}
    </aside>
  );
}

/* ════════ Main App ════════ */
function App() {
  const [arwFiles, setArwFiles] = useState<string[]>([]);
  const [folderPath, setFolderPath] = useState<string | null>(null);
  const [errorMsg, setErrorMsg] = useState("");
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [previewPath, setPreviewPath] = useState<string | null>(null);
  const [loadedCount, setLoadedCount] = useState(0);
  const gridRef = useRef<HTMLDivElement>(null);
  const { width, height } = useContainerSize(gridRef);

  // Track load progress
  useEffect(() => {
    if (arwFiles.length === 0) return;
    const interval = setInterval(() => { setLoadedCount(arwFiles.filter((f) => thumbnailCache.has(f)).length); }, 500);
    return () => clearInterval(interval);
  }, [arwFiles]);

  async function openFolder() {
    try {
      setErrorMsg("");
      const selected = await open({ directory: true, multiple: false, title: "Select ARW Folder" });
      if (!selected) return;
      const dirPath = selected as string;
      setArwFiles([]); thumbnailCache.clear(); queue = []; activeCount = 0;
      setSelectedPath(null); setPreviewPath(null);
      const files: string[] = await invoke("list_arw_files", { dirPath });
      if (files.length === 0) { setErrorMsg("No .ARW files found."); }
      else { setArwFiles(files); setFolderPath(dirPath); }
    } catch (e) { setErrorMsg(String(e)); }
  }

  function handleClickItem(path: string) {
    if (selectedPath === path) {
      // Double-click same item = open preview
      setPreviewPath(path);
    } else {
      setSelectedPath(path);
    }
  }

  const previewIndex = previewPath ? arwFiles.indexOf(previewPath) : -1;
  const goPrev = useCallback(() => { if (previewIndex > 0) setPreviewPath(arwFiles[previewIndex - 1]); }, [previewIndex, arwFiles]);
  const goNext = useCallback(() => { if (previewIndex < arwFiles.length - 1) setPreviewPath(arwFiles[previewIndex + 1]); }, [previewIndex, arwFiles]);
  const closePreview = useCallback(() => setPreviewPath(null), []);

  const folderName = folderPath?.split(/[\\/]/).pop() || "";

  return (
    <>
      {/* ─── Toolbar ─── */}
      <div className="toolbar">
        <div className="toolbar-left">
          <h1>AlphaView</h1>
          <span className="app-subtitle">Library</span>
        </div>
        <div className="toolbar-center">
          <button className="btn btn-primary" onClick={openFolder}>📁 Open Folder</button>
        </div>
        <div className="toolbar-right">
          {arwFiles.length > 0 && (
            <span style={{ fontSize: "11px", color: "var(--text-dim)" }}>
              {loadedCount}/{arwFiles.length} loaded
            </span>
          )}
        </div>
      </div>

      {/* ─── Main 3-panel layout ─── */}
      <div className="main-layout">
        {/* Left sidebar */}
        <aside className="sidebar">
          <div className="sidebar-section">
            <h3>Folders</h3>
            {folderPath ? (
              <div className="sidebar-item active" title={folderPath}>{folderName}</div>
            ) : (
              <div className="sidebar-info">No folder opened</div>
            )}
          </div>
          <div className="sidebar-section">
            <h3>Quick Info</h3>
            <div className="sidebar-info">{arwFiles.length > 0 ? `${arwFiles.length} ARW files` : "—"}</div>
          </div>
        </aside>

        {/* Center: Grid */}
        <div className="content-area">
          {errorMsg && (
            <div className="content-header">
              <span style={{ color: "var(--danger)", fontWeight: "bold" }}>{errorMsg}</span>
            </div>
          )}
          <div className="grid-container" ref={gridRef}>
            {arwFiles.length > 0 && width > 0 && height > 0 ? (
              <VirtualGrid
                items={arwFiles}
                containerWidth={width}
                containerHeight={height}
                colWidth={200}
                rowHeight={220}
                selectedPath={selectedPath}
                onClickItem={handleClickItem}
              />
            ) : (
              <div className="empty-state">
                <div className="icon">📷</div>
                <div className="message">Click "Open Folder" to load ARW images</div>
              </div>
            )}
          </div>
        </div>

        {/* Right: Metadata panel */}
        <MetadataPanel path={selectedPath} />
      </div>

      {/* Preview overlay */}
      {previewPath && <PreviewOverlay path={previewPath} onClose={closePreview} onPrev={goPrev} onNext={goNext} />}
    </>
  );
}

export default App;
