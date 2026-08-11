/* Spindle Dashboard client-side auth bootstrap.
 *
 * The bearer token lives only in localStorage and is attached to every
 * outbound htmx request (including hx-boost page turns) as `X-Api-Token`,
 * which the dashboard then proxies to the Spindle REST API. No session state
 * is held server-side, so any number of dashboard instances can be
 * load-balanced.
 */
(function () {
  'use strict';
  var STORAGE_KEY = 'spindle_token';

  function getToken() { try { return localStorage.getItem(STORAGE_KEY) || ''; } catch (e) { return ''; } }
  function setToken(t) { try { localStorage.setItem(STORAGE_KEY, t); } catch (e) {} }
  function clearToken() { try { localStorage.removeItem(STORAGE_KEY); } catch (e) {} }

  function escapeHtml(s) {
    return String(s).replace(/[&<>"']/g, function (c) {
      return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
    });
  }

  function updateAuthStatus() {
    var el = document.getElementById('auth-status');
    if (!el) return;
    var tok = getToken();
    if (tok) {
      el.innerHTML =
        '<span class="connected">● Connected (' + escapeHtml(String(tok.length)) + ' chars)</span>' +
        ' <button id="disconnect-btn" class="btn">Disconnect</button>';
      var b = el.querySelector('#disconnect-btn');
      if (b) b.addEventListener('click', function (e) {
        e.preventDefault();
        clearToken();
        updateAuthStatus();
        if (window.htmx) htmx.ajax('GET', '/dashboard', { target: 'body', swap: 'innerHTML' });
      });
    } else {
      el.innerHTML = '<a href="/login" class="muted">Not connected — enter API token</a>';
    }
  }

  // Attach the current token to every htmx request (link boosts + partials).
  document.addEventListener('htmx:configRequest', function (evt) {
    var t = getToken();
    if (t) evt.detail.headers['X-Api-Token'] = t;
  });

  // Refresh the auth status widget whenever htmx swaps content.
  document.addEventListener('htmx:afterSwap', function () { updateAuthStatus(); });

  document.addEventListener('DOMContentLoaded', function () {
    updateAuthStatus();

    var form = document.getElementById('login-form');
    if (form) {
      form.addEventListener('submit', function (e) {
        e.preventDefault();
        var input = document.getElementById('token-input');
        var tok = (input && input.value) ? input.value.trim() : '';
        if (!tok) return;
        setToken(tok);
        if (window.htmx) {
          htmx.ajax('GET', '/dashboard', {
            target: 'body',
            swap: 'innerHTML',
            headers: { 'X-Api-Token': tok }
          });
          try { history.replaceState(null, '', '/dashboard'); } catch (e) {}
        } else {
          window.location.href = '/dashboard';
        }
      });
    }
  });
})();
