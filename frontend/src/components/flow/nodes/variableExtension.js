import { Node, mergeAttributes } from "@tiptap/core";

/**
 * Atomic inline node for variables.
 *
 * Stored as `<var data-var-name="x" data-var-type="String">x</var>` so the
 * backend can resolve `data-var-name` at runtime. Rendered as a chip.
 */
export const Variable = Node.create({
    name: "variable",
    group: "inline",
    inline: true,
    atom: true,
    selectable: true,
    draggable: true,

    addAttributes() {
        return {
            "data-var-name": {
                default: null,
                parseHTML: (el) => el.getAttribute("data-var-name"),
                renderHTML: (attrs) => ({ "data-var-name": attrs["data-var-name"] }),
            },
            "data-var-type": {
                default: null,
                parseHTML: (el) => el.getAttribute("data-var-type"),
                renderHTML: (attrs) =>
                    attrs["data-var-type"]
                        ? { "data-var-type": attrs["data-var-type"] }
                        : {},
            },
        };
    },

    parseHTML() {
        return [
            {
                tag: "var[data-var-name]",
                getAttrs: (el) => {
                    const name = el.getAttribute("data-var-name");
                    if (!name) return false;
                    return { "data-var-name": name };
                },
            },
        ];
    },

    renderHTML({ node, HTMLAttributes }) {
        return [
            "var",
            mergeAttributes(HTMLAttributes, { class: "var-chip" }),
            node.attrs["data-var-name"],
        ];
    },

    renderText({ node }) {
        return node.attrs["data-var-name"];
    },

    addCommands() {
        return {
            insertVariable:
                (name, vtype) =>
                ({ chain, state }) => {
                    if (!name) return false;
                    return chain()
                        .insertContent({
                            type: this.name,
                            attrs: {
                                "data-var-name": name,
                                "data-var-type": vtype || null,
                            },
                        })
                        .run();
                },
        };
    },
});
