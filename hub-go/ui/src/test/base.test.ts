import { appPath, baseHref, routerBasename, url } from '../lib/base';
import { setBase } from './helpers';

describe('base path', () => {
  it('works at the root', () => {
    setBase('/');
    expect(baseHref()).toBe('/');
    expect(routerBasename()).toBe('/');
    expect(url('api/auth/me')).toBe('/api/auth/me');
    expect(appPath('http://localhost:3000/m/a/p/b?view=terminal')).toBe('/m/a/p/b?view=terminal');
  });
  it('works under a proxy prefix', () => {
    setBase('/klark/');
    expect(routerBasename()).toBe('/klark');
    expect(url('api/auth/me')).toBe('/klark/api/auth/me');
    expect(url('/api/machines')).toBe('/klark/api/machines'); // a leading slash never escapes the prefix
    expect(appPath('/klark/m/a/f?path=%2Fx')).toBe('/m/a/f?path=%2Fx');
    expect(appPath('/klark')).toBe('/');
  });
  it('does not claim other prefixes or origins', () => {
    setBase('/klark/');
    expect(appPath('/other/m/a')).toBeNull();
    expect(appPath('https://evil.example/klark/m/a')).toBeNull();
  });
  it('accepts a base without a trailing slash', () => {
    setBase('/klark');
    expect(url('api/x')).toBe('/klark/api/x');
    expect(routerBasename()).toBe('/klark');
  });
});
