import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { resizeImage } from '../ui/image';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const TINY_B64 = 'AAAA'; // Minimal valid-ish base64; actual decoding is mocked

const blobObj = new Blob(['pixels'], { type: 'image/jpeg' });

beforeEach(() => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue({ blob: () => Promise.resolve(blobObj) }));
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

// ---------------------------------------------------------------------------
// Mock factories — use ES6 classes so `new X()` works with `vi.stubGlobal`
// ---------------------------------------------------------------------------

function mockOffscreenCanvas(opts: {
  convertToBlob?: ((o: any) => Promise<Blob>) | undefined;
  getContext?: () => any;
} = {}) {
  const drawImage = vi.fn();

  return class MockOffscreenCanvas {
    width: number;
    height: number;
    constructor(w: number, h: number) {
      this.width = w;
      this.height = h;
    }
    getContext() {
      return opts.getContext ? opts.getContext() : { drawImage };
    }
    convertToBlob = opts.convertToBlob;
  };
}

function mockImageBitmap(w: number, h: number) {
  return { width: w, height: h, close: vi.fn() } as unknown as ImageBitmap;
}

function mockFileReader(base64Result: string) {
  return class MockFileReader {
    result: string | null = null;
    onload: (() => void) | null = null;
    onerror: ((e: any) => void) | null = null;
    readAsDataURL(_: Blob) {
      this.result = `data:image/jpeg;base64,${base64Result}`;
      setTimeout(() => this.onload?.(), 0);
    }
  };
}

function mockCreateImageBitmap(bitmap: ImageBitmap) {
  return vi.fn().mockResolvedValue(bitmap);
}

function mockCanvasViaCreateElement() {
  const drawImage = vi.fn();
  const canvas = {
    width: 0, height: 0,
    getContext: vi.fn(() => ({ drawImage })),
    toDataURL: vi.fn(() => 'data:image/jpeg;base64,CANVAS_RESULT'),
  };
  vi.spyOn(document, 'createElement').mockReturnValue(canvas as any);
  return { canvas, drawImage };
}

// ---------------------------------------------------------------------------
// Path 1: createImageBitmap + OffscreenCanvas.convertToBlob
// ---------------------------------------------------------------------------
describe('resizeImage — OffscreenCanvas path', () => {
  it('returns base64 via convertToBlob when all APIs available', async () => {
    const bitmap = mockImageBitmap(2000, 1000);
    vi.stubGlobal('createImageBitmap', mockCreateImageBitmap(bitmap));
    vi.stubGlobal('FileReader', mockFileReader('OFFSCREEN_OUT'));

    const convertToBlob = vi.fn().mockResolvedValue(new Blob([], { type: 'image/jpeg' }));
    vi.stubGlobal('OffscreenCanvas', mockOffscreenCanvas({ convertToBlob }));

    const result = await resizeImage(TINY_B64, 'image/jpeg', 1024);

    expect(result).toBe('OFFSCREEN_OUT');
    expect(bitmap.close).toHaveBeenCalled();
    expect(convertToBlob).toHaveBeenCalledWith({ type: 'image/jpeg', quality: 0.8 });
  });

  it('scales dimensions correctly (4000×2000 → 1024×512)', async () => {
    const bitmap = mockImageBitmap(4000, 2000);
    vi.stubGlobal('createImageBitmap', mockCreateImageBitmap(bitmap));
    vi.stubGlobal('FileReader', mockFileReader('X'));

    let capturedW = 0, capturedH = 0;
    const OffscreenCls = class {
      w: number; h: number;
      constructor(w: number, h: number) { this.w = w; this.h = h; capturedW = w; capturedH = h; }
      getContext() { return { drawImage: vi.fn() }; }
      convertToBlob = vi.fn().mockResolvedValue(new Blob([], { type: 'image/jpeg' }));
    };
    vi.stubGlobal('OffscreenCanvas', OffscreenCls);

    await resizeImage(TINY_B64, 'image/jpeg', 1024);

    expect(capturedW).toBe(1024);
    expect(capturedH).toBe(512);
  });

  it('preserves original dimensions when smaller than maxWidth', async () => {
    const bitmap = mockImageBitmap(800, 600);
    vi.stubGlobal('createImageBitmap', mockCreateImageBitmap(bitmap));
    vi.stubGlobal('FileReader', mockFileReader('SMALL'));

    let capturedW = 0, capturedH = 0;
    const OffscreenCls = class {
      constructor(w: number, h: number) { capturedW = w; capturedH = h; }
      getContext() { return { drawImage: vi.fn() }; }
      convertToBlob = vi.fn().mockResolvedValue(new Blob([], { type: 'image/jpeg' }));
    };
    vi.stubGlobal('OffscreenCanvas', OffscreenCls);

    const result = await resizeImage(TINY_B64, 'image/jpeg', 1024);

    expect(capturedW).toBe(800);
    expect(capturedH).toBe(600);
    expect(result).toBe('SMALL');
  });
});

// ---------------------------------------------------------------------------
// Path 2: createImageBitmap + regular canvas (convertToBlob unavailable)
// ---------------------------------------------------------------------------
describe('resizeImage — canvas fallback (no convertToBlob)', () => {
  it('falls back to canvas.toDataURL when convertToBlob is undefined', async () => {
    const bitmap = mockImageBitmap(2000, 1000);
    vi.stubGlobal('createImageBitmap', mockCreateImageBitmap(bitmap));
    vi.stubGlobal('OffscreenCanvas', mockOffscreenCanvas({ convertToBlob: undefined }));

    const { canvas } = mockCanvasViaCreateElement();

    const result = await resizeImage(TINY_B64, 'image/jpeg', 1024);

    expect(result).toBe('CANVAS_RESULT');
    expect(canvas.toDataURL).toHaveBeenCalledWith('image/jpeg', 0.8);
    expect(bitmap.close).toHaveBeenCalled();
  });

  it('falls back to canvas when OffscreenCanvas is completely absent', async () => {
    const bitmap = mockImageBitmap(2000, 1000);
    vi.stubGlobal('createImageBitmap', mockCreateImageBitmap(bitmap));
    vi.stubGlobal('OffscreenCanvas', undefined);

    const { canvas: _canvas } = mockCanvasViaCreateElement();

    const result = await resizeImage(TINY_B64, 'image/jpeg', 1024);

    expect(result).toBe('CANVAS_RESULT');
    expect(bitmap.close).toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// Path 3: Full fallback — no createImageBitmap at all
// ---------------------------------------------------------------------------
describe('resizeImage — Image() fallback', () => {
  it('uses new Image() + canvas when createImageBitmap is absent', async () => {
    vi.stubGlobal('createImageBitmap', undefined);

    // Mock Image constructor as a class
    vi.stubGlobal('Image', class MockImage {
      width = 3000;
      height = 1500;
      onload: (() => void) | null = null;
      onerror: ((e: any) => void) | null = null;
      set src(_: string) {
        // Simulate async image load
        setTimeout(() => this.onload?.(), 0);
      }
    });

    const { canvas } = mockCanvasViaCreateElement();

    const result = await resizeImage(TINY_B64, 'image/jpeg', 1024);

    expect(result).toBe('CANVAS_RESULT');
    expect(canvas.toDataURL).toHaveBeenCalledWith('image/jpeg', 0.8);
  });
});

// ---------------------------------------------------------------------------
// Error handling & cleanup
// ---------------------------------------------------------------------------
describe('resizeImage — error handling', () => {
  it('closes imgBitmap even when convertToBlob rejects', async () => {
    const bitmap = mockImageBitmap(2000, 1000);
    vi.stubGlobal('createImageBitmap', mockCreateImageBitmap(bitmap));

    const convertToBlob = vi.fn().mockRejectedValue(new Error('encode failed'));
    vi.stubGlobal('OffscreenCanvas', mockOffscreenCanvas({ convertToBlob }));

    await expect(resizeImage(TINY_B64, 'image/jpeg', 1024)).rejects.toThrow('encode failed');
    expect(bitmap.close).toHaveBeenCalled();
  });

  it('rethrows errors from data-URI fetch', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('network down')));

    await expect(resizeImage(TINY_B64, 'image/jpeg', 1024)).rejects.toThrow('network down');
  });
});
