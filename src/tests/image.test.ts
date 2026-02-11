import { describe, it, expect } from 'vitest';
import { resizeImage } from '../ui/image';

describe('resizeImage', () => {
  it('should be a function', () => {
    expect(typeof resizeImage).toBe('function');
  });
});
