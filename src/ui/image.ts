/**
 * Image processing utilities — async resizing via createImageBitmap + OffscreenCanvas
 * with progressive fallbacks for environments that lack modern APIs.
 */

/**
 * Resizes an image asynchronously using modern web APIs (createImageBitmap, OffscreenCanvas).
 * This keeps the main thread responsive by offloading decoding and encoding.
 *
 * Fallback chain:
 *  1. createImageBitmap + OffscreenCanvas.convertToBlob  (fastest, fully off-main-thread)
 *  2. createImageBitmap + regular canvas.toDataURL        (decode off-thread, encode on-thread)
 *  3. new Image() + regular canvas.toDataURL              (fully on-thread, broadest compat)
 */
export async function resizeImage(base64: string, mimeType: string, maxWidth: number): Promise<string> {
  try {
    // Convert base64 to Blob efficiently
    const response = await fetch(`data:${mimeType};base64,${base64}`);
    const blob = await response.blob();

    // Calculate target dimensions from the source
    let width: number;
    let height: number;

    // 1. Try createImageBitmap for off-main-thread decoding
    if (typeof createImageBitmap === 'function') {
      const imgBitmap = await createImageBitmap(blob);
      try {
        width = imgBitmap.width;
        height = imgBitmap.height;

        if (width > maxWidth) {
          height = Math.round((height * maxWidth) / width);
          width = maxWidth;
        }

        // 2. Try OffscreenCanvas + convertToBlob for fully off-thread encoding
        if (typeof OffscreenCanvas !== 'undefined') {
          const offscreen = new OffscreenCanvas(width, height);
          const ctx = offscreen.getContext('2d');
          if (!ctx) throw new Error("Could not get offscreen canvas context");

          ctx.drawImage(imgBitmap, 0, 0, width, height);

          // Guard convertToBlob — some browsers expose OffscreenCanvas without it
          if (typeof offscreen.convertToBlob === 'function') {
            const resizedBlob = await offscreen.convertToBlob({
              type: 'image/jpeg',
              quality: 0.8
            });

            return new Promise((resolve, reject) => {
              const reader = new FileReader();
              reader.onload = () => {
                const result = reader.result as string;
                resolve(result.split(',')[1]);
              };
              reader.onerror = reject;
              reader.readAsDataURL(resizedBlob);
            });
          }
          // Fall through to regular canvas encoding if convertToBlob unavailable
        }

        // Fallback: createImageBitmap available but OffscreenCanvas/convertToBlob not
        const canvas = document.createElement("canvas");
        canvas.width = width;
        canvas.height = height;
        const ctx = canvas.getContext("2d");
        if (!ctx) throw new Error("Could not get canvas context");

        ctx.drawImage(imgBitmap, 0, 0, width, height);
        const resizedDataUrl = canvas.toDataURL("image/jpeg", 0.8);
        return resizedDataUrl.split(",")[1];
      } finally {
        imgBitmap.close();
      }
    }

    // 3. Full fallback: no createImageBitmap — use new Image() + canvas
    const img = await new Promise<HTMLImageElement>((resolve, reject) => {
      const image = new Image();
      image.onload = () => resolve(image);
      image.onerror = reject;
      image.src = `data:${mimeType};base64,${base64}`;
    });

    width = img.width;
    height = img.height;

    if (width > maxWidth) {
      height = Math.round((height * maxWidth) / width);
      width = maxWidth;
    }

    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("Could not get canvas context");

    ctx.drawImage(img, 0, 0, width, height);
    const resizedDataUrl = canvas.toDataURL("image/jpeg", 0.8);
    return resizedDataUrl.split(",")[1];
  } catch (error) {
    console.error("Error in resizeImage:", error);
    // Log and rethrow to let callers handle the failure
    throw error;
  }
}
