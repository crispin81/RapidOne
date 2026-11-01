/** Two pictures of one shape, to within 1%. */
export const sameShape = (a: { w: number; h: number }, b: { w: number; h: number }) => Math.abs(a.w / a.h - b.w / b.h) < 0.01;
