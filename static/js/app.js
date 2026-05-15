// Zeptodo client glue.
//
// Exposes window.taskList(): an Alpine component that binds Sortable.js to the
// task list and writes accessible status messages to #reorder-live after each
// drag-and-drop reorder.

(function () {
	"use strict";

	function announce(message) {
		const live = document.getElementById("reorder-live");
		if (!live) return;
		live.textContent = "";
		// Force the live region to re-announce when the same message repeats.
		window.setTimeout(function () {
			live.textContent = message;
		}, 30);
	}

	function rowTitle(row) {
		const span = row.querySelector("[data-task-title]");
		if (span) return span.textContent.trim();
		const fallback = row.querySelector("span.flex-1");
		return fallback ? fallback.textContent.trim() : "task";
	}

	function rowIds(listEl) {
		return Array.prototype.slice
			.call(listEl.querySelectorAll("[data-task-id]"))
			.map(function (el) {
				return el.getAttribute("data-task-id");
			});
	}

	window.taskList = function () {
		return {
			sortable: null,
			init: function (el) {
				const self = this;
				if (typeof Sortable === "undefined") return;
				if (this.sortable) {
					this.sortable.destroy();
					this.sortable = null;
				}
				this.sortable = Sortable.create(el, {
					handle: ".drag-handle",
					draggable: "[data-task-id]:not(.task-row-terminal)",
					animation: 150,
					delay: 200,
					delayOnTouchOnly: true,
					ghostClass: "opacity-50",
					onMove: function (evt) {
						if (
							evt.related &&
							evt.related.classList &&
							evt.related.classList.contains("task-row-terminal")
						) {
							return false;
						}
						return true;
					},
					onEnd: function (evt) {
						if (evt.oldIndex === evt.newIndex) return;
						const csrf = el.getAttribute("data-csrf") || "";
						const ids = rowIds(el).join(",");
						const moved = evt.item;
						const title = rowTitle(moved);
						announce(
							"Moved " + title + " to position " + (evt.newIndex + 1),
						);
						if (typeof htmx !== "undefined") {
							htmx.ajax("POST", "/tasks/reorder", {
								target: "#task-list",
								swap: "outerHTML",
								values: { _csrf: csrf, ids: ids },
							});
						}
					},
				});
			},
		};
	};

})();
