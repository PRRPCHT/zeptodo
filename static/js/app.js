// Zeptodo client glue.
//
// All Alpine components are registered as Alpine.data() factories because
// the app uses Alpine's CSP build, which forbids runtime compilation of
// directive expressions. Templates reference these factories by name in
// x-data="..."; directive expressions can then call methods on the component
// instead of evaluating arbitrary inline JavaScript.

(function () {
	"use strict";

	function announce(message) {
		const live = document.getElementById("reorder-live");
		if (!live) return;
		live.textContent = "";
		// Re-set the message after a tick so identical consecutive messages
		// still get announced.
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

	function isReorderable(row) {
		return row && !row.classList.contains("task-row-terminal");
	}

	function postReorder(listEl) {
		const csrf = listEl.getAttribute("data-csrf") || "";
		const ids = rowIds(listEl).join(",");
		if (typeof htmx === "undefined") return;
		htmx.ajax("POST", "/tasks/reorder", {
			target: "#task-list",
			swap: "outerHTML",
			values: { _csrf: csrf, ids: ids },
		});
	}

	function moveRow(row, direction) {
		if (!isReorderable(row)) return;
		const listEl = row.closest("#task-list");
		if (!listEl) return;
		const candidates = Array.prototype.slice.call(
			listEl.querySelectorAll("[data-task-id]"),
		);
		const index = candidates.indexOf(row);
		if (index < 0) return;
		const targetIndex = index + direction;
		if (targetIndex < 0 || targetIndex >= candidates.length) return;
		const neighbor = candidates[targetIndex];
		if (!isReorderable(neighbor)) return;
		if (direction < 0) {
			neighbor.parentNode.insertBefore(row, neighbor);
		} else {
			neighbor.parentNode.insertBefore(row, neighbor.nextSibling);
		}
		const newPosition =
			Array.prototype.slice
				.call(listEl.querySelectorAll("[data-task-id]"))
				.indexOf(row) + 1;
		const title = rowTitle(row);
		const directionWord = direction < 0 ? "up" : "down";
		announce(
			"Moved " + title + " " + directionWord + " to position " + newPosition,
		);
		const rowId = row.getAttribute("data-task-id");
		function refocus(evt) {
			if (!evt.target || evt.target.id !== "task-list") return;
			document.removeEventListener("htmx:afterSwap", refocus);
			const restored = document.querySelector(
				'[data-task-id="' + rowId + '"]',
			);
			if (restored) restored.focus();
		}
		document.addEventListener("htmx:afterSwap", refocus);
		postReorder(listEl);
	}

	document.addEventListener("alpine:init", function () {
		const Alpine = window.Alpine;

		Alpine.data("taskList", function () {
			return {
				sortable: null,
				init: function () {
					const el = this.$el;
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
							const moved = evt.item;
							const title = rowTitle(moved);
							announce(
								"Moved " + title + " to position " + (evt.newIndex + 1),
							);
							postReorder(el);
						},
					});
				},
			};
		});

		Alpine.data("taskRow", function () {
			return {
				editing: false,
				showDesc: false,
				startEdit: function () {
					this.editing = true;
					if (document.activeElement && document.activeElement.blur) {
						document.activeElement.blur();
					}
				},
				cancelEdit: function () {
					this.editing = false;
				},
				toggleDesc: function () {
					this.showDesc = !this.showDesc;
				},
				moveUp: function () {
					moveRow(this.$el, -1);
				},
				moveDown: function () {
					moveRow(this.$el, 1);
				},
				focusTitleIfEditing: function () {
					const self = this;
					this.$nextTick(function () {
						if (self.editing && self.$el && self.$el.focus) {
							self.$el.focus();
						}
					});
				},
			};
		});

		Alpine.data("headerNav", function () {
			return {
				menuOpen: false,
				toggle: function () {
					this.menuOpen = !this.menuOpen;
				},
				close: function () {
					this.menuOpen = false;
				},
			};
		});

		Alpine.data("themeToggle", function () {
			return {
				flip: function () {
					const html = document.documentElement;
					html.dataset.theme =
						html.dataset.theme === "dark" ? "light" : "dark";
				},
			};
		});

		Alpine.data("createTaskForm", function () {
			return {
				expanded: false,
				toggle: function () {
					this.expanded = !this.expanded;
				},
				onAfterRequest: function (event) {
					if (event && event.detail && event.detail.successful) {
						this.$el.reset();
						this.expanded = false;
					}
				},
			};
		});

		Alpine.data("apiKeyCreated", function () {
			return {
				copied: false,
				copy: function () {
					const input = this.$refs.plaintext;
					if (!input) return;
					if (navigator.clipboard && navigator.clipboard.writeText) {
						navigator.clipboard.writeText(input.value);
					} else {
						input.select();
						try {
							document.execCommand("copy");
						} catch (_e) {
							// best-effort fallback
						}
					}
					const self = this;
					this.copied = true;
					setTimeout(function () {
						self.copied = false;
					}, 1500);
				},
				selectAll: function () {
					if (this.$el && this.$el.select) this.$el.select();
				},
			};
		});

		Alpine.data("apiKeyRow", function () {
			return {
				editingDescription: false,
				editingExpiry: false,
				focusDescriptionIfEditing: function () {
					const self = this;
					this.$nextTick(function () {
						if (self.editingDescription && self.$el && self.$el.focus) {
							self.$el.focus();
						}
					});
				},
			};
		});
	});
})();
