/* ================================================================
   HCDF Website — nav.js
   Dropdown menus, collapsible sections, hamburger, active page.
   ================================================================ */

document.addEventListener("DOMContentLoaded", function () {

  /* --- Dropdown menus (hover on desktop, click on mobile) --- */
  var dropdowns = document.querySelectorAll(".dropdown");

  dropdowns.forEach(function (dd) {
    var toggle = dd.querySelector(":scope > a");

    /* Click toggles on any screen */
    toggle.addEventListener("click", function (e) {
      e.preventDefault();
      var wasOpen = dd.classList.contains("open");
      closeAllDropdowns();
      if (!wasOpen) dd.classList.add("open");
    });

    /* Hover open/close on wider screens */
    dd.addEventListener("mouseenter", function () {
      if (window.innerWidth > 768) {
        dd.classList.add("open");
      }
    });
    dd.addEventListener("mouseleave", function () {
      if (window.innerWidth > 768) {
        dd.classList.remove("open");
      }
    });
  });

  function closeAllDropdowns() {
    dropdowns.forEach(function (d) { d.classList.remove("open"); });
  }

  /* Close dropdowns on outside click */
  document.addEventListener("click", function (e) {
    if (!e.target.closest(".dropdown")) {
      closeAllDropdowns();
    }
  });

  /* --- Hamburger menu --- */
  var hamburger = document.querySelector(".hamburger");
  var navLinks = document.querySelector(".nav-links");

  if (hamburger && navLinks) {
    hamburger.addEventListener("click", function () {
      hamburger.classList.toggle("open");
      navLinks.classList.toggle("open");
    });
  }

  /* --- Collapsible sections --- */
  var collapsibles = document.querySelectorAll(".collapsible-header");

  collapsibles.forEach(function (header) {
    header.addEventListener("click", function () {
      var parent = header.closest(".collapsible");
      var indicator = header.querySelector(".indicator");
      parent.classList.toggle("open");
      if (indicator) {
        indicator.textContent = parent.classList.contains("open") ? "\u2212" : "+";
      }
    });
  });

  /* --- Active page highlighting --- */
  var path = window.location.pathname;
  /* Remove trailing index.html */
  path = path.replace(/index\.html$/, "");

  var navAnchors = document.querySelectorAll("nav .nav-links a[href]");
  navAnchors.forEach(function (a) {
    var href = a.getAttribute("href");
    if (!href || href.startsWith("http") || href === "#") return;
    var normalized = href.replace(/index\.html$/, "");
    if (path === normalized || (normalized !== "/" && path.startsWith(normalized))) {
      a.classList.add("active");
      /* If inside a dropdown, mark the dropdown trigger too */
      var dd = a.closest(".dropdown");
      if (dd) dd.classList.add("active");
    }
  });

  /* --- Anchor scroll for hash links (examples page) --- */
  if (window.location.hash) {
    var target = document.querySelector(window.location.hash);
    if (target) {
      var coll = target.closest(".collapsible");
      if (coll && !coll.classList.contains("open")) {
        coll.classList.add("open");
        var ind = coll.querySelector(".indicator");
        if (ind) ind.textContent = "\u2212";
      }
      setTimeout(function () {
        target.scrollIntoView({ behavior: "smooth", block: "start" });
      }, 100);
    }
  }
});
