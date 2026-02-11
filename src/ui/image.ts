/**
 * Image processing utilities
 */

/**
 * Resizes an image asynchronously using modern web APIs (createImageBitmap, OffscreenCanvas).
 * This keeps the main thread responsive by offloading decoding and encoding.
 */
export async function resizeImage(base64: string, mimeType: string, maxWidth: number): Promise<string> {
  try {
    // Convert base64 to Blob efficiently
    const response = await fetch(`data:${mimeType};base64,${base64}`);
    const blob = await response.blob();

    // 1. Decode image off-main-thread using createImageBitmap
    const imgBitmap = await createImageBitmap(blob);

    try {
      let width = imgBitmap.width;
      let height = imgBitmap.height;

      if (width > maxWidth) {
        height = Math.round((height * maxWidth) / width);
        width = maxWidth;
      }

      // 2. Use OffscreenCanvas if available for async resizing and encoding
      if (typeof OffscreenCanvas !== 'undefined') {
        const offscreen = new OffscreenCanvas(width, height);
        const ctx = offscreen.getContext('2d');
        if (!ctx) throw new Error("Could not get offscreen canvas context");

        ctx.drawImage(imgBitmap, 0, 0, width, height);

        // 3. Encode off-main-thread using convertToBlob
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
      } else {
        // Fallback to regular canvas if OffscreenCanvas is not supported
        const canvas = document.createElement("canvas");
        canvas.width = width;
        canvas.height = height;
        const ctx = canvas.getContext("2d");
        if (!ctx) throw new Error("Could not get canvas context");

        ctx.drawImage(imgBitmap, 0, 0, width, height);
        const resizedDataUrl = canvas.toDataURL("image/jpeg", 0.8);
        return resizedDataUrl.split(",")[1];
      }
    } finally {
      imgBitmap.close();
    }
  } catch (error) {
    console.error("Error in resizeImage:", error);
    // Fallback or rethrow
    throw error;
  }
}
